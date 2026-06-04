//! Прямая UDP-сессия хоста по EVRT-протоколу.
//!
//! Работает поверх punch-hole адреса, полученного от hbbs RustDesk.
//! Если прямое UDP недоступно — вызывающий код падает обратно на TCP relay.
//!
//! # Схема
//! ```text
//! hbbs ──punch-hole──► peer_addr (UDP)
//!                            │
//!                     EVRT handshake
//!                     (SessionConfig → CodecConfig → VideoFrames)
//!                            │
//!                     FeedbackLoop ◄── клиент
//!                     AdaptiveRelief ──► encoder reconfigure
//! ```

use std::{
    net::{SocketAddr, UdpSocket},
    sync::{
        atomic::{AtomicBool, AtomicU32, Ordering},
        mpsc::{self, Receiver, Sender},
        Arc, Mutex,
    },
    thread,
    time::{Duration, Instant},
};

use crate::{
    evrt::{self, ControlMessage, Pressure, ReceiverFeedback, SessionConfig},
    frame_queue::{AdaptiveJitter, AdaptiveRelief, ChannelReassembler, FrameQueue, FrameQueueConfig},
    host::{HostEvent, HostCommand},
    settings::AppConfig,
};

// ─── константы ────────────────────────────────────────────────────────────────

/// Таймаут ожидания первого пакета от клиента.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);
/// Таймаут молчания перед разрывом сессии.
const IDLE_TIMEOUT: Duration = Duration::from_secs(8);
/// Интервал keepalive (session config повтор).
const KEEPALIVE_INTERVAL: Duration = Duration::from_secs(2);
/// Размер приёмного UDP буфера.
const RECV_BUF: usize = 512 * 1024;
/// Размер передающего UDP буфера.
const SEND_BUF: usize = 128 * 1024;
/// Интервал feedback цикла (ultra low latency).
const FEEDBACK_INTERVAL_ULL: Duration = Duration::from_millis(70);
/// Интервал feedback цикла (normal).
const FEEDBACK_INTERVAL_NORMAL: Duration = Duration::from_millis(150);

// ─── публичный интерфейс ──────────────────────────────────────────────────────

/// Параметры для запуска прямой EVRT-сессии.
pub struct EvrtSessionParams {
    /// Адрес клиента (получен от hbbs через punch-hole).
    pub peer_addr: SocketAddr,
    /// Наш UDP-сокет (тот же что использовался для регистрации на hbbs).
    pub socket: Arc<UdpSocket>,
    /// Конфиг приложения.
    pub config: AppConfig,
    /// ID пира (для логов и событий).
    pub peer_id: String,
    /// Канал событий → UI.
    pub events: Sender<HostEvent>,
    /// Команды от UI.
    pub stop: Arc<AtomicBool>,
    /// Текущий target FPS (может меняться).
    pub target_fps: Arc<AtomicU32>,
    /// Текущее качество (quality_milli).
    pub quality_milli: Arc<AtomicU32>,
}

/// Запустить прямую EVRT-сессию в отдельных потоках.
/// Блокирует до завершения сессии.
pub fn run_evrt_session(params: EvrtSessionParams) -> Result<(), String> {
    let EvrtSessionParams {
        peer_addr,
        socket,
        config,
        peer_id,
        events,
        stop,
        target_fps,
        quality_milli,
    } = params;

    evrt_log(&events, format!("EVRT session starting → {peer_addr}"));

    // Настройки буферов сокета
    let _ = socket.set_write_timeout(Some(Duration::from_millis(200)));
    // set_send_buffer_size недоступен через Arc<UdpSocket> — пропускаем (OS default достаточен)

    // ── Построить и отправить SessionConfig ──────────────────────────────────
    let (screen_w, screen_h) = crate::capture::screen_size().unwrap_or((1920, 1080));
    let fps = target_fps.load(Ordering::Relaxed).clamp(5, 60);
    let bitrate = crate::host::h264_target_bitrate_bps_pub(screen_w, screen_h, fps, quality_milli.load(Ordering::Relaxed));

    let session_cfg = SessionConfig {
        codec:           choose_codec_label(&config),
        preset:          if config.display.fsr_quality.is_enabled() { "GAME".into() } else { "MEDIA".into() },
        width:           screen_w,
        height:          screen_h,
        fps,
        bitrate,
        stream_mode:     "single".into(),
        adaptation_mode: "GAME".into(),
    };

    let cfg_pkt = evrt::build_session_config(&session_cfg.to_json());
    send_udp(&socket, &cfg_pkt, peer_addr)?;
    evrt_log(&events, format!(
        "EVRT SessionConfig sent: {}x{}@{} bitrate={:.1}Mbps codec={}",
        screen_w, screen_h, fps,
        bitrate as f64 / 1_000_000.0,
        session_cfg.codec,
    ));

    // ── Подтверждение подключения: ждём первый пакет от клиента ──────────────
    let deadline = Instant::now() + CONNECT_TIMEOUT;
    socket.set_read_timeout(Some(Duration::from_millis(200))).ok();
    let mut buf = vec![0u8; evrt::MAX_PACKET_SIZE + 64];
    let mut confirmed = false;

    while Instant::now() < deadline && !stop.load(Ordering::Relaxed) {
        match socket.recv_from(&mut buf) {
            Ok((len, src)) if src == peer_addr => {
                if let Some(pkt) = evrt::parse(&buf, len) {
                    match evrt::parse_control(&pkt.payload) {
                        Some(ControlMessage::RequestKeyFrame) => {
                            evrt_log(&events, "EVRT: client connected (RequestKeyFrame received)".into());
                            confirmed = true;
                            break;
                        }
                        _ => {
                            // Любой пакет от правильного адреса = подтверждение
                            confirmed = true;
                            break;
                        }
                    }
                }
            }
            Ok(_) => {} // пакет от другого адреса
            Err(ref e) if is_would_block(e) => {}
            Err(e) => return Err(format!("EVRT connect wait: {e}")),
        }
    }

    if !confirmed {
        return Err("EVRT: client did not respond in time".into());
    }

    // ── Feedback-канал: клиент → adaptive relief ──────────────────────────────
    let (feedback_tx, feedback_rx) = mpsc::channel::<ReceiverFeedback>();

    // ── Канал для keyframe-запросов (из feedback loop → encoder) ─────────────
    let (keyframe_tx, keyframe_rx) = mpsc::channel::<()>();

    // ── Поток приёма UDP (control + feedback) ────────────────────────────────
    let recv_socket  = socket.clone();
    let recv_stop    = stop.clone();
    let recv_peer    = peer_addr;
    let recv_fb_tx   = feedback_tx;
    let recv_kf_tx   = keyframe_tx;
    let recv_events  = events.clone();

    let recv_handle = thread::spawn(move || {
        let mut buf = vec![0u8; evrt::MAX_PACKET_SIZE + 64];
        recv_socket.set_read_timeout(Some(Duration::from_millis(500))).ok();
        let mut last_packet_at = Instant::now();

        while !recv_stop.load(Ordering::Relaxed) {
            match recv_socket.recv_from(&mut buf) {
                Ok((len, src)) if src == recv_peer => {
                    last_packet_at = Instant::now();
                    if let Some(pkt) = evrt::parse(&buf, len) {
                        if pkt.packet_type == evrt::TYPE_CONTROL {
                            match evrt::parse_control(&pkt.payload) {
                                Some(ControlMessage::RequestKeyFrame) => {
                                    let _ = recv_kf_tx.send(());
                                }
                                Some(ControlMessage::ReceiverFeedback(fb)) => {
                                    let _ = recv_fb_tx.send(fb);
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

            if last_packet_at.elapsed() > IDLE_TIMEOUT {
                evrt_log(&recv_events, "EVRT: idle timeout — client disconnected".into());
                recv_stop.store(true, Ordering::Relaxed);
                break;
            }
        }
    });

    // ── Основной цикл: захват + кодирование + отправка ────────────────────────
    // Здесь порт ConsiderAdaptiveRelief + feedback loop
    let mut relief = AdaptiveRelief::new(true);
    let mut frame_id: u32 = 0;
    let mut last_keyframe_at = Instant::now();
    let mut last_keepalive_at = Instant::now();
    let mut current_bitrate = bitrate;
    let mut current_fps = fps;

    // Захват кадров (те же механизмы что в video_loop, но отправка через UDP)
    let mut mf_encoder: Option<crate::mf_encode::MfVideoEncoder> = None;
    let mut mf_disabled = false;

    let codec_pref = config.display.codec;
    let encoder_pref = config.display.encoder;
    let client_video = crate::host::ClientVideoSupport {
        h264: true,
        h265: true,
        av1:  false,
        prefer: crate::rustdesk_proto::PreferCodec::Auto,
    };
    let desired_codec = crate::host::choose_mf_encoder_codec_pub(encoder_pref, codec_pref, client_video);

    // Буфер захвата (shared с capture thread)
    type CaptureSlot = Arc<Mutex<Option<(u32, u32, Vec<u8>)>>>;
    let cap_slot: CaptureSlot = Arc::new(Mutex::new(None));
    let cap_stop   = stop.clone();
    let cap_fps    = target_fps.clone();
    let cap_slot_bg = cap_slot.clone();

    thread::spawn(move || {
        let mut buf = Vec::new();
        loop {
            if cap_stop.load(Ordering::Relaxed) { break; }
            let fps = cap_fps.load(Ordering::Relaxed).clamp(5, 60);
            if let Some((w, h)) = crate::capture::capture_screen_into(&mut buf) {
                if let Ok(mut slot) = cap_slot_bg.lock() {
                    match slot.as_mut() {
                        Some(s) if s.0 == w && s.1 == h => {
                            std::mem::swap(&mut s.2, &mut buf);
                        }
                        _ => *slot = Some((w, h, buf.clone())),
                    }
                }
            }
            let budget = Duration::from_micros(1_000_000 / fps.max(1) as u64);
            thread::sleep(budget.saturating_sub(Duration::from_micros(500)));
        }
    });

    let _ = events.send(HostEvent::SessionStarted { peer_id: peer_id.clone() });

    evrt_log(&events, "EVRT: capture loop running".into());

    let mut last_frame_at = Instant::now();

    while !stop.load(Ordering::Relaxed) {
        // ── Обработать feedback ───────────────────────────────────────────────
        while let Ok(fb) = feedback_rx.try_recv() {
            let cur_fps = target_fps.load(Ordering::Relaxed);
            if let Some(new_step) = relief.on_feedback(&fb, cur_fps) {
                let scale = AdaptiveRelief::bitrate_scale(new_step);
                current_bitrate = (bitrate as f32 * scale) as u32;
                evrt_log(&events, format!(
                    "EVRT adaptive relief: step={new_step} bitrate={:.1}Mbps scale={scale:.2}",
                    current_bitrate as f64 / 1_000_000.0,
                ));
            }
        }

        // ── Обработать запрос keyframe ────────────────────────────────────────
        let force_key = keyframe_rx.try_recv().is_ok()
            || last_keyframe_at.elapsed() > Duration::from_secs(
                (2.max(60 / current_fps.max(1))) as u64
            );

        // ── Keepalive: повтор SessionConfig ──────────────────────────────────
        if last_keepalive_at.elapsed() > KEEPALIVE_INTERVAL {
            let updated_cfg = SessionConfig {
                bitrate: current_bitrate,
                fps:     current_fps,
                ..session_cfg.clone()
            };
            let pkt = evrt::build_session_config(&updated_cfg.to_json());
            let _ = send_udp(&socket, &pkt, peer_addr);
            last_keepalive_at = Instant::now();
        }

        // ── Тайминг кадров ────────────────────────────────────────────────────
        current_fps = target_fps.load(Ordering::Relaxed).clamp(5, 60);
        let frame_budget = Duration::from_micros(1_000_000 / current_fps as u64);
        let elapsed = last_frame_at.elapsed();
        if elapsed < frame_budget {
            let remaining = frame_budget - elapsed;
            if remaining > Duration::from_micros(2_000) {
                thread::sleep(remaining - Duration::from_micros(500));
            }
            continue;
        }
        last_frame_at = Instant::now();

        // ── Захватить кадр ────────────────────────────────────────────────────
        let Some((cap_w, cap_h, capture_bgra)) =
            cap_slot.lock().ok().and_then(|mut s| s.take())
        else {
            thread::sleep(Duration::from_millis(1));
            continue;
        };

        let quality = quality_milli.load(Ordering::Relaxed);
        let bitrate_now = crate::host::h264_target_bitrate_bps_pub(
            cap_w, cap_h, current_fps,
            quality,
        );
        // Применить adaptive scale
        let effective_bitrate = (bitrate_now as f32 * AdaptiveRelief::bitrate_scale(relief.current_step())) as u32;

        // ── Закодировать ──────────────────────────────────────────────────────
        let encoded = if let Some(codec) = desired_codec.filter(|_| !mf_disabled) {
            match crate::host::encode_mf_frame_pub(
                &mut mf_encoder, codec, cap_w, cap_h, current_fps,
                effective_bitrate, &capture_bgra, force_key,
            ) {
                Ok(Some(pkt)) => {
                    frame_id = frame_id.wrapping_add(1);
                    Some((pkt.bytes, pkt.key, frame_id))
                }
                Ok(None) => None,
                Err(e) => {
                    evrt_log(&events, format!("EVRT MF encode error: {e}, disabling"));
                    mf_disabled = true;
                    None
                }
            }
        } else {
            // Fallback: PNG один раз в секунду (нет H264 SW без feature)
            if last_keyframe_at.elapsed() > Duration::from_secs(1) {
                let png = encode_png_fallback(&capture_bgra, cap_w, cap_h);
                frame_id = frame_id.wrapping_add(1);
                Some((png, true, frame_id))
            } else {
                None
            }
        };

        // ── Отправить кадр ────────────────────────────────────────────────────
        if let Some((payload, is_key, fid)) = encoded {
            if is_key {
                // Перед keyframe отправляем CodecConfig (SPS/PPS если есть)
                if let Some(ref enc) = mf_encoder {
                    if let Some(cfg_bytes) = enc.codec_config() {
                        let cfg_pkt = evrt::build_codec_config(&cfg_bytes);
                        let _ = send_udp(&socket, &cfg_pkt, peer_addr);
                    }
                }
                last_keyframe_at = Instant::now();
            }

            let pts = evrt::now_us();
            let packets = evrt::packetize_video_frame(fid, pts, is_key, &payload);
            for pkt in &packets {
                if let Err(e) = send_udp(&socket, pkt, peer_addr) {
                    evrt_log(&events, format!("EVRT send error: {e}"));
                    stop.store(true, Ordering::Relaxed);
                    break;
                }
            }
        }
    }

    let _ = events.send(HostEvent::SessionEnded {
        peer_id: peer_id.clone(),
        reason:  "EVRT session ended".into(),
    });

    evrt_log(&events, format!("EVRT session ended for {peer_id}"));
    let _ = recv_handle.join();

    // Освободить застрявшие кнопки
    crate::host::release_stuck_input_pub();

    Ok(())
}

// ─── вспомогательные ──────────────────────────────────────────────────────────

fn send_udp(socket: &UdpSocket, data: &[u8], addr: SocketAddr) -> Result<(), String> {
    socket.send_to(data, addr).map_err(|e| format!("UDP send: {e}"))?;
    Ok(())
}

fn is_would_block(e: &std::io::Error) -> bool {
    e.kind() == std::io::ErrorKind::WouldBlock
        || e.kind() == std::io::ErrorKind::TimedOut
}

fn choose_codec_label(config: &AppConfig) -> String {
    match config.display.codec {
        crate::settings::CodecPreference::H265 => "H265",
        _                                      => "H264",
    }
    .to_owned()
}

fn encode_png_fallback(bgra: &[u8], w: u32, h: u32) -> Vec<u8> {
    // Очень простой PNG через image crate (уже есть в зависимостях).
    use image::{ImageBuffer, Rgba};
    let img: ImageBuffer<Rgba<u8>, Vec<u8>> = ImageBuffer::from_raw(
        w, h, bgra.to_vec(),
    ).unwrap_or_else(|| ImageBuffer::new(w, h));
    let mut png = Vec::new();
    let mut cursor = std::io::Cursor::new(&mut png);
    img.write_to(&mut cursor, image::ImageFormat::Png).ok();
    png
}

fn evrt_log(events: &Sender<HostEvent>, msg: String) {
    eprintln!("[evrt] {msg}");
    let _ = events.send(HostEvent::Log(msg));
}

// ─── Парсинг socket_addr из protobuf bytes ────────────────────────────────────

/// Декодировать `socket_addr: Vec<u8>` из RustDesk protobuf в `SocketAddr`.
///
/// Формат hbbs: 4 байта IPv4 + 2 байта port (big-endian), или
///              16 байт IPv6 + 2 байта port.
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
            let ip = Ipv6Addr::from(ip_bytes);
            let port = u16::from_be_bytes([bytes[16], bytes[17]]);
            Some(SocketAddr::new(IpAddr::V6(ip), port))
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
        let bytes = [192, 168, 1, 100, 0x1F, 0x90]; // 192.168.1.100:8080
        let addr = decode_punch_addr(&bytes).unwrap();
        assert_eq!(addr.port(), 8080);
        assert_eq!(addr.to_string(), "192.168.1.100:8080");
    }

    #[test]
    fn decode_punch_addr_invalid() {
        assert!(decode_punch_addr(&[1, 2, 3]).is_none());
        assert!(decode_punch_addr(&[]).is_none());
    }
}
