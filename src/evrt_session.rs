// =============================================================================
// EVRT Protocol — разработан Артуром Валиевым (Artur Valiev)
// Оригинальная реализация: EvertyGame (C#, https://github.com/djvaliev)
// Rust-порт для EvertyDesk Lite выполнен на основе оригинальных алгоритмов.
//
// Протокол, алгоритмы адаптивной буферизации, система давления (pressure),
// логика FeedbackLoop и LatestAccessUnitQueue — интеллектуальная собственность
// Артура Валиева, разработанная в течение нескольких лет работы над EvertyGame.
// =============================================================================

//! Прямая UDP-сессия хоста по EVRT-протоколу.
//!
//! КРИТИЧНО: использует тот же `Arc<UdpSocket>` что зарегистрирован на hbbs.
//! Именно этот порт сообщается клиенту — punch-hole работает только с ним.
//!
//! # Поток данных
//! ```text
//! hbbs punch-hole → peer_addr (UDP клиента)
//!
//! Хост                                    Клиент
//! ──────────────────────────────────────────────────────
//! 3× UDP punch (открыть NAT клиента)
//! SessionConfig ──────────────────────────────────────►
//! CodecConfig ────────────────────────────────────────►
//!                        ◄──────────── RequestKeyFrame
//! IDR frame ──────────────────────────────────────────►
//! frame N ────────────────────────────────────────────►
//! frame N+1 ──────────────────────────────────────────►
//!                        ◄──────── ReceiverFeedback(70ms)
//! AdaptiveRelief ← pressure
//! keepalive SessionConfig ────────────────────────────► (2s)
//! ```

use std::{
    net::{SocketAddr, UdpSocket},
    sync::{
        atomic::{AtomicBool, AtomicU32, Ordering},
        mpsc::{self, Sender},
        Arc, Mutex,
    },
    thread,
    time::{Duration, Instant},
};

use crate::{
    evrt::{self, ControlMessage, ReceiverFeedback, SessionConfig},
    frame_queue::AdaptiveRelief,
    host::HostEvent,
    settings::AppConfig,
};

// ─── константы ────────────────────────────────────────────────────────────────

/// Таймаут ожидания первого пакета от клиента после отправки SessionConfig.
const CONNECT_TIMEOUT:      Duration = Duration::from_secs(5);
/// Таймаут молчания — клиент не слал ничего X сек → разрыв.
const IDLE_TIMEOUT:         Duration = Duration::from_secs(8);
/// Интервал повтора SessionConfig (keepalive + обновление bitrate).
const KEEPALIVE_INTERVAL:   Duration = Duration::from_secs(2);
/// Минимальный интервал между принудительными IDR (при отсутствии запросов).
const IDR_MIN_INTERVAL:     Duration = Duration::from_secs(2);
/// Высокоточный sleep threshold — ниже этого используем spin-loop.
const SPIN_THRESHOLD:       Duration = Duration::from_micros(1_500);

// ─── публичный интерфейс ──────────────────────────────────────────────────────

/// Параметры для запуска прямой EVRT-сессии.
pub struct EvrtSessionParams {
    /// Адрес клиента (получен от hbbs через punch-hole).
    pub peer_addr: SocketAddr,
    /// ★ Тот же UDP-сокет что использован для регистрации на hbbs.
    ///   Именно с этого порта hbbs сообщил клиенту наш адрес.
    pub socket: Arc<UdpSocket>,
    /// Конфиг приложения.
    pub config: AppConfig,
    /// ID пира (для логов).
    pub peer_id: String,
    /// Канал событий → UI.
    pub events: Sender<HostEvent>,
    /// Сигнал остановки (из UI или при ошибке).
    pub stop: Arc<AtomicBool>,
    /// Целевой FPS (может меняться во время сессии).
    pub target_fps: Arc<AtomicU32>,
    /// Quality milli (1000 = 100%, 700 = 70%).
    pub quality_milli: Arc<AtomicU32>,
}

/// Запустить прямую EVRT-сессию. Блокирует до завершения.
pub fn run_evrt_session(params: EvrtSessionParams) -> Result<(), String> {
    let EvrtSessionParams {
        peer_addr, socket, config, peer_id, events, stop, target_fps, quality_milli,
    } = params;

    // ── Windows performance hints ─────────────────────────────────────────────
    // timeBeginPeriod(1): системный таймер 1 мс вместо 15.6 мс
    // ProcessPriority::High: кодировщик не вытесняется фоном
    let _perf = WindowsPerfHints::enable(&events);

    evrt_log(&events, format!("EVRT session starting → {peer_addr}"));

    // ── SessionConfig ─────────────────────────────────────────────────────────
    let (screen_w, screen_h) = crate::capture::screen_size().unwrap_or((1920, 1080));
    let fps     = target_fps.load(Ordering::Relaxed).clamp(5, 60);
    let quality = quality_milli.load(Ordering::Relaxed);
    let bitrate = crate::host::h264_target_bitrate_bps_pub(screen_w, screen_h, fps, quality);

    let session_cfg = SessionConfig {
        codec:           choose_codec(&config),
        preset:          if config.display.fsr_quality.is_enabled() { "GAME".into() } else { "MEDIA".into() },
        width:           screen_w,
        height:          screen_h,
        fps,
        bitrate,
        stream_mode:     "single".into(),
        adaptation_mode: "GAME".into(),
    };

    // Отправляем SessionConfig 2 раза чтобы компенсировать возможную потерю первого UDP
    let cfg_pkt = evrt::build_session_config(&session_cfg.to_json());
    send_udp(&socket, &cfg_pkt, peer_addr)?;
    thread::sleep(Duration::from_millis(5));
    send_udp(&socket, &cfg_pkt, peer_addr)?;

    evrt_log(&events, format!(
        "EVRT SessionConfig ×2: {}×{}@{} {:.1}Mbps codec={}",
        screen_w, screen_h, fps,
        bitrate as f64 / 1_000_000.0,
        session_cfg.codec,
    ));

    // ── Ожидаем подтверждение от клиента (RequestKeyFrame) ───────────────────
    socket.set_read_timeout(Some(Duration::from_millis(200))).ok();
    let mut buf       = vec![0u8; evrt::MAX_PACKET_SIZE + 64];
    let deadline      = Instant::now() + CONNECT_TIMEOUT;
    let mut confirmed = false;

    while Instant::now() < deadline && !stop.load(Ordering::Relaxed) {
        match socket.recv_from(&mut buf) {
            Ok((len, src)) if src == peer_addr => {
                evrt_log(&events, "EVRT: client response received".into());
                confirmed = true;
                // Обработаем этот пакет ниже если это RequestKeyFrame
                let _ = (len, src); // suppress unused
                break;
            }
            Ok(_) | Err(_) if is_would_block_err() => {}
            Err(e) => return Err(format!("EVRT connect: {e}")),
            _ => {}
        }
        // Повтор SessionConfig каждые 200 мс пока ждём
        send_udp(&socket, &cfg_pkt, peer_addr)?;
    }

    if !confirmed {
        return Err(format!("EVRT: клиент {peer_addr} не ответил за {CONNECT_TIMEOUT:?}"));
    }

    // ── Feedback + keyframe request каналы ───────────────────────────────────
    let (fb_tx,  fb_rx)  = mpsc::channel::<ReceiverFeedback>();
    let (kf_tx,  kf_rx)  = mpsc::channel::<()>();

    // ── Receive loop: control/feedback пакеты от клиента ─────────────────────
    let recv_sock   = socket.clone();
    let recv_stop   = stop.clone();
    let recv_events = events.clone();

    let recv_handle = thread::spawn(move || {
        let mut buf = vec![0u8; evrt::MAX_PACKET_SIZE + 64];
        recv_sock.set_read_timeout(Some(Duration::from_millis(500))).ok();
        let mut last_pkt = Instant::now();

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
                evrt_log(&recv_events, "EVRT: клиент молчит — idle timeout".into());
                recv_stop.store(true, Ordering::Relaxed);
                break;
            }
        }
    });

    // ── Encoder + capture ─────────────────────────────────────────────────────
    let mut mf_enc:      Option<crate::mf_encode::MfVideoEncoder> = None;
    let mut mf_disabled  = false;
    let mut relief       = AdaptiveRelief::new(true);
    let mut frame_id:    u32    = 0;
    let mut current_bps: u32    = bitrate;
    let mut current_fps: u32    = fps;

    // PTS: монотонный счётчик в единицах 100 нс (HNS), как в EvertyGame
    let pts_start = Instant::now();
    let hns_per_frame = |fps: u32| -> u64 { 10_000_000 / fps.max(1) as u64 };
    let mut sample_time_hns: u64 = 0;
    let sample_dur_hns  = hns_per_frame(fps);

    let codec_pref   = config.display.codec;
    let encoder_pref = config.display.encoder;
    let client_video = crate::host::ClientVideoSupport { h264: true, h265: true, av1: false,
        prefer: crate::rustdesk_proto::PreferCodec::Auto };
    let desired_codec = crate::host::choose_mf_encoder_codec_pub(encoder_pref, codec_pref, client_video);

    // Capture thread — двойной буфер
    type CapSlot = Arc<Mutex<Option<(u32, u32, Vec<u8>)>>>;
    let cap_slot: CapSlot = Arc::new(Mutex::new(None));
    {
        let cap_stop   = stop.clone();
        let cap_fps    = target_fps.clone();
        let cap_slot_t = cap_slot.clone();
        thread::spawn(move || {
            let mut buf = Vec::new();
            loop {
                if cap_stop.load(Ordering::Relaxed) { break; }
                let fps = cap_fps.load(Ordering::Relaxed).clamp(5, 60);
                if let Some((w, h)) = crate::capture::capture_screen_into(&mut buf) {
                    if let Ok(mut slot) = cap_slot_t.lock() {
                        match slot.as_mut() {
                            Some(s) if s.0 == w && s.1 == h => std::mem::swap(&mut s.2, &mut buf),
                            _ => *slot = Some((w, h, buf.clone())),
                        }
                    }
                }
                let frame_us = 1_000_000u64 / fps.max(1) as u64;
                thread::sleep(Duration::from_micros(frame_us.saturating_sub(500)));
            }
        });
    }

    let _ = events.send(HostEvent::SessionStarted { peer_id: peer_id.clone() });
    evrt_log(&events, "EVRT: encode loop started".into());

    // Отслеживаем nextFrameDueTicks как в EvertyGame WindowsSenderSession
    let mut next_frame_due = Instant::now();
    let mut last_keepalive = Instant::now();
    let mut last_idr       = Instant::now();
    let mut last_fb_drops: u64 = 0;

    while !stop.load(Ordering::Relaxed) {

        // ── Обработать feedback ───────────────────────────────────────────────
        while let Ok(fb) = fb_rx.try_recv() {
            let cur_fps = target_fps.load(Ordering::Relaxed);
            if let Some(step) = relief.on_feedback(&fb, cur_fps) {
                let scale = AdaptiveRelief::bitrate_scale(step);
                current_bps = (bitrate as f32 * scale) as u32;
                evrt_log(&events, format!(
                    "EVRT adaptive relief step={step} → {:.1}Mbps (×{scale:.2})",
                    current_bps as f64 / 1_000_000.0,
                ));
                // Отправить обновлённый SessionConfig клиенту
                let upd = SessionConfig { bitrate: current_bps, fps: current_fps, ..session_cfg.clone() };
                let _ = send_udp(&socket, &evrt::build_session_config(&upd.to_json()), peer_addr);
            }
        }

        // ── Keyframe request ──────────────────────────────────────────────────
        let want_idr = kf_rx.try_recv().is_ok()
            || last_idr.elapsed() > IDR_MIN_INTERVAL;

        // ── Keepalive ─────────────────────────────────────────────────────────
        if last_keepalive.elapsed() > KEEPALIVE_INTERVAL {
            let upd = SessionConfig { bitrate: current_bps, fps: current_fps, ..session_cfg.clone() };
            let _ = send_udp(&socket, &evrt::build_session_config(&upd.to_json()), peer_addr);
            last_keepalive = Instant::now();
        }

        // ── Точный тайминг кадров (порт nextFrameDueTicks из EvertyGame) ──────
        current_fps = target_fps.load(Ordering::Relaxed).clamp(5, 60);
        let frame_interval = Duration::from_nanos(1_000_000_000 / current_fps as u64);

        let now = Instant::now();
        if now < next_frame_due {
            let wait = next_frame_due - now;
            if wait > SPIN_THRESHOLD {
                thread::sleep(wait - SPIN_THRESHOLD);
            } else {
                std::hint::spin_loop();
            }
            continue;
        }
        // Продвигаем на следующий дедлайн (catchup: если отстали — прыгаем вперёд)
        next_frame_due += frame_interval;
        if next_frame_due < Instant::now() {
            next_frame_due = Instant::now() + frame_interval;
        }

        // ── Захват ────────────────────────────────────────────────────────────
        let Some((cap_w, cap_h, bgra)) =
            cap_slot.lock().ok().and_then(|mut s| s.take())
        else {
            thread::sleep(Duration::from_millis(1));
            continue;
        };

        // ── Битрейт с adaptive scale ──────────────────────────────────────────
        let eff_bps = (crate::host::h264_target_bitrate_bps_pub(
            cap_w, cap_h, current_fps, quality_milli.load(Ordering::Relaxed),
        ) as f32 * AdaptiveRelief::bitrate_scale(relief.current_step())) as u32;

        // ── Кодирование ───────────────────────────────────────────────────────
        let encoded: Option<(Vec<u8>, bool)> =
            if let Some(codec) = desired_codec.filter(|_| !mf_disabled) {
                match crate::host::encode_mf_frame_pub(
                    &mut mf_enc, codec, cap_w, cap_h, current_fps, eff_bps, &bgra, want_idr,
                ) {
                    Ok(Some(pkt)) => Some((pkt.bytes, pkt.key)),
                    Ok(None)      => None,
                    Err(e) => {
                        evrt_log(&events, format!("EVRT encode error: {e}"));
                        mf_disabled = true;
                        None
                    }
                }
            } else {
                // PNG fallback (нет H264 SW)
                if want_idr {
                    Some((encode_png_fallback(&bgra, cap_w, cap_h), true))
                } else {
                    None
                }
            };

        // ── Отправить ─────────────────────────────────────────────────────────
        if let Some((payload, is_idr)) = encoded {
            frame_id = frame_id.wrapping_add(1);

            if is_idr {
                // CodecConfig (SPS/PPS) перед IDR — точно как в EvertyGame
                if let Some(ref enc) = mf_enc {
                    if let Some(sps_pps) = enc.codec_config() {
                        let _ = send_udp(&socket, &evrt::build_codec_config(&sps_pps), peer_addr);
                    }
                }
                last_idr = Instant::now();
            }

            // PresentationTimeUs в микросекундах = HNS / 10
            // Используем sample_time_hns как в EvertyGame (_sampleTimeHns)
            let pts_us = sample_time_hns / 10;
            sample_time_hns = sample_time_hns.wrapping_add(hns_per_frame(current_fps));

            let pkts = evrt::packetize_video_frame(frame_id, pts_us, is_idr, &payload);
            for pkt in &pkts {
                if send_udp(&socket, pkt, peer_addr).is_err() {
                    stop.store(true, Ordering::Relaxed);
                    break;
                }
            }
        }
    }

    let _ = events.send(HostEvent::SessionEnded {
        peer_id: peer_id.clone(),
        reason:  "EVRT".into(),
    });
    let _ = events.send(HostEvent::StateChanged(crate::host::HostState::Ready));

    evrt_log(&events, format!("EVRT: сессия с {peer_id} завершена"));
    let _ = recv_handle.join();
    crate::host::release_stuck_input_pub();
    Ok(())
}

// ─── Windows performance hints ────────────────────────────────────────────────
// Порт WindowsPerformanceHints.cs из EvertyGame:
//   timeBeginPeriod(1) — таймер 1 мс
//   ProcessPriorityClass::High — приоритет выше фона
// Автоматически восстанавливается в Drop.

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

            // timeBeginPeriod(1)
            #[link(name = "winmm")]
            extern "system" { fn timeBeginPeriod(uPeriod: c_uint) -> c_uint; }
            let timer_raised = unsafe { timeBeginPeriod(1) } == 0;
            if timer_raised {
                evrt_log(events, "EVRT perf: timer resolution → 1 ms ✓".into());
            }

            // SetPriorityClass(HIGH_PRIORITY_CLASS = 0x80)
            #[link(name = "kernel32")]
            extern "system" {
                fn GetCurrentProcess() -> *mut std::ffi::c_void;
                fn SetPriorityClass(hProcess: *mut std::ffi::c_void, dwPriorityClass: c_uint) -> i32;
                fn GetPriorityClass(hProcess: *mut std::ffi::c_void) -> c_uint;
            }
            let proc = unsafe { GetCurrentProcess() };
            let orig = unsafe { GetPriorityClass(proc) };
            let set  = unsafe { SetPriorityClass(proc, 0x80) }; // HIGH_PRIORITY_CLASS
            if set != 0 {
                evrt_log(events, "EVRT perf: process priority → High ✓".into());
            }

            Self { timer_raised, original_priority: Some(orig) }
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
                extern "system" { fn timeEndPeriod(uPeriod: c_uint) -> c_uint; }
                unsafe { timeEndPeriod(1); }
            }

            if let Some(orig) = self.original_priority {
                #[link(name = "kernel32")]
                extern "system" {
                    fn GetCurrentProcess() -> *mut std::ffi::c_void;
                    fn SetPriorityClass(hProcess: *mut std::ffi::c_void, dwPriorityClass: c_uint) -> i32;
                }
                unsafe { SetPriorityClass(GetCurrentProcess(), orig); }
            }
        }
    }
}

// ─── вспомогательные ──────────────────────────────────────────────────────────

fn send_udp(socket: &UdpSocket, data: &[u8], addr: SocketAddr) -> Result<(), String> {
    socket.send_to(data, addr).map_err(|e| format!("UDP send: {e}"))?;
    Ok(())
}

fn is_would_block(e: &std::io::Error) -> bool {
    e.kind() == std::io::ErrorKind::WouldBlock || e.kind() == std::io::ErrorKind::TimedOut
}

fn is_would_block_err() -> bool { false } // placeholder для match guard

fn choose_codec(config: &AppConfig) -> String {
    match config.display.codec {
        crate::settings::CodecPreference::H265 => "H265",
        _ => "H264",
    }.to_owned()
}

fn encode_png_fallback(bgra: &[u8], w: u32, h: u32) -> Vec<u8> {
    use image::{ImageBuffer, Rgba};
    let img: ImageBuffer<Rgba<u8>, Vec<u8>> =
        ImageBuffer::from_raw(w, h, bgra.to_vec())
            .unwrap_or_else(|| ImageBuffer::new(w, h));
    let mut png = Vec::new();
    img.write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png).ok();
    png
}

fn evrt_log(events: &Sender<HostEvent>, msg: String) {
    eprintln!("[evrt] {msg}");
    let _ = events.send(HostEvent::Log(msg));
}

/// Декодировать `socket_addr: Vec<u8>` из RustDesk protobuf в `SocketAddr`.
/// Формат hbbs: 4 байта IPv4 + 2 байта port (big-endian).
pub fn decode_punch_addr(bytes: &[u8]) -> Option<SocketAddr> {
    use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};
    match bytes.len() {
        6 => {
            let ip   = Ipv4Addr::new(bytes[0], bytes[1], bytes[2], bytes[3]);
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

// ─── тесты ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decode_punch_addr_v4() {
        let bytes = [192u8, 168, 1, 100, 0x1F, 0x90];
        let addr = decode_punch_addr(&bytes).unwrap();
        assert_eq!(addr.port(), 8080);
        assert_eq!(addr.to_string(), "192.168.1.100:8080");
    }

    #[test]
    fn decode_punch_addr_invalid() {
        assert!(decode_punch_addr(&[]).is_none());
        assert!(decode_punch_addr(&[1, 2, 3]).is_none());
    }

    #[test]
    fn pts_hns_monotonic() {
        // HNS PTS должен монотонно расти на hns_per_frame
        let fps = 60u64;
        let hpf = 10_000_000 / fps;
        let mut pts: u64 = 0;
        for _ in 0..100 {
            let us = pts / 10;
            pts = pts.wrapping_add(hpf);
            assert!(us < pts / 10 || pts == 0); // монотонность
        }
        // При 60fps шаг = 166_666 нс = 16_666.6 мкс
        assert_eq!(hpf, 166_666);
    }
}
