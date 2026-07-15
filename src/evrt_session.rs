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
use socket2::SockRef;

// ─── константы ────────────────────────────────────────────────────────────────

const IDLE_TIMEOUT: Duration = Duration::from_secs(8);
const KEEPALIVE_INTERVAL: Duration = Duration::from_secs(2);
const SPIN_THRESHOLD: Duration = Duration::from_micros(100);
const PACER_BURST_PACKETS: u8 = 4;

// ─── публичный интерфейс ──────────────────────────────────────────────────────

/// Параметры EVRT UDP сессии.
///
/// Сессия не захватывает экран и не кодирует — всё это делает `video_pipeline`.
/// Здесь только UDP-доставка + feedback.
pub struct EvrtSessionParams {
    /// Адрес клиента (после punch-hole).
    pub peer_addr: SocketAddr,
    pub session_token: Option<String>,
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
    /// Запрос немедленного IDR в общий encoder pipeline.
    pub idr_request_tx: Sender<()>,
}

/// Запустить EVRT сессию. Блокирует до завершения.
pub fn run_evrt_session(params: EvrtSessionParams) -> Result<(), String> {
    let EvrtSessionParams {
        peer_addr,
        session_token,
        socket,
        config,
        peer_id,
        events,
        stop,
        frame_rx,
        target_fps,
        quality_milli,
        bitrate_scale_milli,
        idr_request_tx,
    } = params;

    // ── Windows performance hints ─────────────────────────────────────────────
    let _perf = WindowsPerfHints::enable(&events);

    evrt_log(&events, format!("EVRT session → {peer_addr}"));
    bitrate_scale_milli.store(1_000, Ordering::Relaxed);
    let socket_ref = SockRef::from(socket.as_ref());
    let _ = socket_ref.set_send_buffer_size(4 * 1024 * 1024);
    let _ = socket_ref.set_recv_buffer_size(512 * 1024);
    if let Ok(size) = socket_ref.send_buffer_size() {
        evrt_log(&events, format!("EVRT host UDP send buffer: {size} bytes"));
    }

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
        codec: "EVRTCK".to_owned(),
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
    let cfg_pkt =
        evrt::build_session_config_authenticated(&session_cfg.to_json(), session_token.as_deref());
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

    let mut ctrl_wait_pkts = 0u32;
    loop {
        if stop.load(Ordering::Relaxed) {
            evrt_log(&events, format!("EVRT TYPE_CTRL wait: stop set after {ctrl_wait_pkts} pkts"));
            return Err("stopped before client connected".into());
        }
        if Instant::now() > deadline {
            evrt_log(&events, format!("EVRT TYPE_CTRL wait: TIMEOUT after {ctrl_wait_pkts} pkts — client {peer_addr} silent"));
            return Err(format!("client {peer_addr} did not respond"));
        }
        match socket.recv_from(&mut buf) {
            Ok((len, src)) if src == peer_addr => {
                ctrl_wait_pkts += 1;
                let parsed = evrt::parse_authenticated(&buf, len, session_token.as_deref());
                let ptype = parsed.as_ref().map(|p| p.packet_type);
                let confirmed = parsed.is_some_and(|pkt| {
                    pkt.packet_type == evrt::TYPE_CONTROL
                        && evrt::control_token_matches(&pkt.payload, session_token.as_deref())
                        && evrt::parse_control(&pkt.payload).is_some()
                });
                if confirmed {
                    evrt_log(&events, format!("EVRT: client confirmed after {ctrl_wait_pkts} pkts"));
                    break;
                }
                evrt_log(&events, format!("EVRT TYPE_CTRL wait: pkt#{ctrl_wait_pkts} len={len} ptype={ptype:?} auth={} — not confirmed",
                    session_token.is_some()));
                send_udp(&socket, &cfg_pkt, peer_addr)?; // retry
            }
            Ok((len, src)) => {
                evrt_log(&events, format!("EVRT TYPE_CTRL wait: pkt from wrong src={src} (expected {peer_addr}) len={len}"));
                send_udp(&socket, &cfg_pkt, peer_addr)?; // retry
            }
            Err(_) => {
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
        let recv_session_token = session_token.clone();

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
                        if let Some(pkt) = evrt::parse_authenticated(
                            &buf,
                            len,
                            recv_session_token.as_deref(),
                        ) {
                            if pkt.packet_type == evrt::TYPE_CONTROL {
                                if !evrt::control_token_matches(
                                    &pkt.payload,
                                    recv_session_token.as_deref(),
                                ) {
                                    continue;
                                }
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
        let audio_session_token = session_token.clone();
        thread::spawn(move || {
            crate::evrt_audio::run_audio_capture(
                audio_sock,
                peer_addr,
                audio_stop,
                audio_session_token,
            );
        });
    }

    // ── Главный цикл: берём кадры из pipeline → пакетизируем → UDP ───────────
    let mut relief = AdaptiveRelief::new(true);
    let mut last_keepalive = Instant::now();
    // EVRTCK — lossless LAN codec; keyframes are 5-10 MB, so we must pace at
    // LAN speed (1 Gbps) rather than the H264-derived network bitrate (3-5 Mbps)
    // which would take 10+ seconds per keyframe and trigger the client idle timeout.
    let is_evrtck = session_cfg.codec.eq_ignore_ascii_case("EVRTCK");
    // EVRTCK keyframes are 5-10 MB (lossless), so pacing must be fast enough to
    // deliver them before the 6s idle timeout, but slow enough not to overflow the
    // client's 4 MB kernel UDP buffer (4MB / 1200 bytes = ~3495 packets max in-flight).
    // At 100 Mbps: 6.7 MB in ~535 ms. Receiver processes at ~120 MB/s → no overflow.
    let mut pacing_bps: u32 = if is_evrtck { 100_000_000 } else { bitrate.max(1) };
    let mut pacer = UdpPacer::new();
    let mut waiting_for_idr = true;
    // For EVRTCK: rate-limit client-triggered IDR requests. Each IDR is ~1 MB;
    // H264 decode errors on the TCP path can cause a RefreshVideoDisplay storm
    // that would otherwise generate a new 1 MB keyframe every 200 ms.
    let mut last_kf_request = Instant::now() - Duration::from_secs(60);
    const IDR_RATELIMIT_EVRTCK: Duration = Duration::from_millis(500);

    // FEC включён по умолчанию; отключается через EVERTYDESK_FEC=0 (диагностика).
    let fec_enabled = std::env::var("EVERTYDESK_FEC")
        .map(|v| v != "0" && !v.eq_ignore_ascii_case("off") && !v.eq_ignore_ascii_case("false"))
        .unwrap_or(true);

    // HNS PTS (как в EvertyGame _sampleTimeHns)
    let mut sample_hns: u64 = 0;
    let hns_per_frame = |fps: u32| 10_000_000u64 / fps.max(1) as u64;

    evrt_log(&events, "EVRT: main loop started".into());
    let _ = idr_request_tx.send(());

    while !stop.load(Ordering::Relaxed) {
        // ── Feedback от клиента ───────────────────────────────────────────────
        // EVRTCK is a lossless delta codec — bitrate/fps scaling has no meaning.
        // Applying adaptive relief would reduce bitrate_scale_milli and generate
        // spurious log noise without any benefit. Drain feedback but don't act on it.
        if is_evrtck {
            while fb_rx.try_recv().is_ok() {}
        } else {
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
        }

        // ── Keyframe request ──────────────────────────────────────────────────
        let mut kf_requested = false;
        while kf_rx.try_recv().is_ok() {
            kf_requested = true;
        }
        if kf_requested && !waiting_for_idr {
            // For EVRTCK, rate-limit to IDR_RATELIMIT_EVRTCK: H264 decode errors
            // on the TCP relay can spam RefreshVideoDisplay → RequestKeyFrame,
            // causing a ~1 MB IDR storm. Non-EVRTCK codecs: send immediately.
            let allowed = !is_evrtck || last_kf_request.elapsed() >= IDR_RATELIMIT_EVRTCK;
            if allowed {
                waiting_for_idr = true;
                last_kf_request = Instant::now();
                let _ = idr_request_tx.send(());
            }
        }

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
            // EVRTCK: keep 100 Mbps pacing regardless of H264-derived bitrate.
            pacing_bps = if is_evrtck { 100_000_000 } else { cur_bps.max(1) };
            let upd = SessionConfig {
                fps: cur_fps,
                bitrate: cur_bps,
                ..session_cfg.clone()
            };
            let _ = send_udp(
                &socket,
                &evrt::build_session_config_authenticated(
                    &upd.to_json(),
                    session_token.as_deref(),
                ),
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

        // После запроса не посылаем зависимые P-кадры до нового IDR.
        if waiting_for_idr && !frame.is_idr {
            continue;
        }

        if frame.is_idr {
            waiting_for_idr = false;
            evrt_log(&events, format!("EVRT: sending IDR frame id={} bytes={} → {peer_addr}", frame.frame_id, frame.bytes.len()));
            // CodecConfig (SPS/PPS) перед IDR
            if let Some(ref sps_pps) = frame.sps_pps {
                let config_packet =
                    evrt::build_codec_config_authenticated(sps_pps, session_token.as_deref());
                let _ = send_udp(&socket, &config_packet, peer_addr);
                thread::sleep(Duration::from_millis(1));
                let _ = send_udp(&socket, &config_packet, peer_addr);
            }
        }

        // ── ROI перед кадром ─────────────────────────────────────────────────
        let roi = evrt::RoiRect {
            frame_id: frame.frame_id,
            ..frame.roi
        };
        let roi_pkt = evrt::build_roi_metadata_authenticated(roi, session_token.as_deref());
        if !roi_pkt.is_empty() {
            let _ = send_udp(&socket, &roi_pkt, peer_addr);
        }

        // ── PTS ───────────────────────────────────────────────────────────────
        let cur_fps = target_fps.load(Ordering::Relaxed).clamp(5, 60);
        let pts_us = sample_hns / 10;
        sample_hns = sample_hns.wrapping_add(hns_per_frame(cur_fps));

        // ── Пакетизация и отправка ────────────────────────────────────────────
        // FEC включён → согласованная пакетизация (чанки с запасом под parity).
        // Иначе — обычная пакетизация полным размером (минимум пакетов).
        let (pkts, fec_pkts) = if fec_enabled {
            evrt::packetize_video_with_fec(
                frame.frame_id,
                pts_us,
                frame.is_idr,
                &frame.bytes,
                session_token.as_deref(),
            )
        } else {
            (
                evrt::packetize_video_frame_authenticated(
                    frame.frame_id,
                    pts_us,
                    frame.is_idr,
                    &frame.bytes,
                    session_token.as_deref(),
                ),
                Vec::new(),
            )
        };

        let mut send_failed = false;
        for pkt in &pkts {
            if pacer.send(&socket, pkt, peer_addr, pacing_bps).is_err() {
                stop.store(true, Ordering::Relaxed);
                send_failed = true;
                break;
            }
        }
        // ── FEC: parity-пакеты после data ─────────────────────────────────────
        // Восстанавливают единичные потери без ретрансмиссии.
        if !send_failed {
            for pkt in &fec_pkts {
                if pacer.send(&socket, pkt, peer_addr, pacing_bps).is_err() {
                    stop.store(true, Ordering::Relaxed);
                    break;
                }
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

struct UdpPacer {
    next_send_at: Instant,
    packets_in_burst: u8,
}

impl UdpPacer {
    fn new() -> Self {
        Self {
            next_send_at: Instant::now(),
            packets_in_burst: 0,
        }
    }

    fn send(
        &mut self,
        socket: &UdpSocket,
        data: &[u8],
        addr: SocketAddr,
        target_bps: u32,
    ) -> Result<(), String> {
        if self.packets_in_burst == 0 {
            let now = Instant::now();
            if self.next_send_at > now {
                precise_wait(self.next_send_at - now);
            } else if now.duration_since(self.next_send_at) > Duration::from_millis(50) {
                self.next_send_at = now;
            }
        }

        send_udp(socket, data, addr)?;

        // 20% headroom accounts for UDP/IP framing and prevents the pacer from
        // falling behind the encoder's configured payload bitrate.
        self.next_send_at += packet_spacing(data.len(), target_bps);
        self.packets_in_burst = (self.packets_in_burst + 1) % PACER_BURST_PACKETS;
        Ok(())
    }
}

fn packet_spacing(bytes: usize, target_bps: u32) -> Duration {
    let wire_bps = u64::from(target_bps.max(1)).saturating_mul(120) / 100;
    let spacing_ns = (bytes as u64)
        .saturating_mul(8)
        .saturating_mul(1_000_000_000)
        / wire_bps.max(1);
    Duration::from_nanos(spacing_ns.max(1))
}

fn precise_wait(wait: Duration) {
    let deadline = Instant::now() + wait;
    if wait > SPIN_THRESHOLD {
        thread::sleep(wait - SPIN_THRESHOLD);
    }
    while Instant::now() < deadline {
        std::hint::spin_loop();
    }
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

    #[test]
    fn packet_pacing_includes_transport_headroom() {
        let spacing = packet_spacing(1_200, 12_000_000);
        assert_eq!(spacing.as_nanos(), 666_666);
    }
}
