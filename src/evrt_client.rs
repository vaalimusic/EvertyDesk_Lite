// =============================================================================
// EVRT Protocol — разработан Артуром Валиевым (Artur Valiev)
// Оригинальная реализация: EvertyGame (C#, https://github.com/djvaliev)
// Rust-порт для EvertyDesk Lite выполнен на основе оригинальных алгоритмов.
//
// Протокол, алгоритмы адаптивной буферизации, система давления (pressure),
// логика FeedbackLoop и LatestAccessUnitQueue — интеллектуальная собственность
// Артура Валиева, разработанная в течение нескольких лет работы над EvertyGame.
// =============================================================================

//! EVRT клиент — прямое UDP подключение к хосту.
//!
//! # Как работает
//!
//! ```text
//! 1. Клиент отправляет PunchHoleRequest(force_relay=false, udp_port=N) → hbbs
//! 2. hbbs сообщает хосту addr клиента (PunchHole), клиенту addr хоста (PunchHoleResponse)
//! 3. Оба одновременно шлют UDP-пакеты → дыра в NAT открыта
//! 4. Клиент отправляет EVRT RequestKeyFrame → хост видит клиента
//! 5. Хост отвечает SessionConfig + CodecConfig + VideoFrames
//! 6. Клиент собирает кадры через ChannelReassembler → декодирует → SessionEvent::Frame
//! 7. FeedbackLoop каждые 70-150мс: ReceiverFeedback { pressure, deltas } → хост
//! ```

use std::{
    net::{SocketAddr, UdpSocket},
    sync::{
        atomic::{AtomicBool, AtomicI32, AtomicU64, Ordering},
        mpsc::Sender,
        Arc,
    },
    thread,
    time::{Duration, Instant},
};

use crate::{
    evrt::{self, ControlMessage, Pressure, ReceiverFeedback, SessionConfig},
    frame_queue::{AdaptiveJitter, ChannelReassembler, FrameQueue, FrameQueueConfig},
    transport::SessionEvent,
};

// ─── константы ────────────────────────────────────────────────────────────────

const CONNECT_TIMEOUT:    Duration = Duration::from_secs(4);
const IDLE_TIMEOUT:       Duration = Duration::from_secs(6);
const FEEDBACK_INTERVAL_ULL: Duration = Duration::from_millis(70);
const FEEDBACK_INTERVAL_NORM: Duration = Duration::from_millis(150);
const PUNCH_REPEATS: usize  = 3;
const PUNCH_GAP:  Duration  = Duration::from_millis(30);

// ─── публичный интерфейс ──────────────────────────────────────────────────────

/// Параметры EVRT-клиента.
pub struct EvrtClientParams {
    /// UDP-сокет клиента (уже забиндированный на локальном порту).
    pub socket:    Arc<UdpSocket>,
    /// Внешний адрес хоста из rendezvous ответа.
    pub host_addr: SocketAddr,
    /// Канал событий → UI.
    pub events:    Sender<SessionEvent>,
    /// Сигнал остановки.
    pub stop:      Arc<AtomicBool>,
    /// Ultra-low-latency режим (feedback каждые 70мс вместо 150мс).
    pub ultra_low_latency: bool,
}

/// Результат попытки EVRT-подключения.
pub enum EvrtConnectResult {
    /// Подключение успешно — сессия завершена нормально.
    Ok,
    /// Хост не ответил в течение таймаута — нужен TCP relay fallback.
    NoResponse,
    /// Сессия оборвалась с ошибкой.
    Error(String),
}

/// Запустить прямую EVRT-сессию клиента. Блокирует до завершения.
pub fn run_evrt_client(params: EvrtClientParams) -> EvrtConnectResult {
    let EvrtClientParams { socket, host_addr, events, stop, ultra_low_latency } = params;

    evrt_log(&events, format!("EVRT client: punching to {host_addr}"));

    // ── UDP punch-hole ────────────────────────────────────────────────────────
    for _ in 0..PUNCH_REPEATS {
        let _ = socket.send_to(&[0u8], host_addr);
        thread::sleep(PUNCH_GAP);
    }

    // Отправляем RequestKeyFrame — хост по нему определяет что клиент живой
    let kf_pkt = evrt::build_request_key_frame();
    let _ = socket.send_to(&kf_pkt, host_addr);

    // ── Ожидаем SessionConfig от хоста ───────────────────────────────────────
    socket.set_read_timeout(Some(Duration::from_millis(300))).ok();
    let mut buf = vec![0u8; evrt::MAX_PACKET_SIZE + 64];
    let deadline = Instant::now() + CONNECT_TIMEOUT;
    let mut session_cfg: Option<SessionConfig> = None;

    while Instant::now() < deadline && !stop.load(Ordering::Relaxed) {
        match socket.recv_from(&mut buf) {
            Ok((len, src)) if src == host_addr => {
                if let Some(pkt) = evrt::parse(&buf, len) {
                    if pkt.packet_type == evrt::TYPE_SESSION_CONFIG {
                        if let Some(cfg) = SessionConfig::from_json(&pkt.payload) {
                            evrt_log(&events, format!(
                                "EVRT: SessionConfig received — {}x{}@{} {} {:.1}Mbps",
                                cfg.width, cfg.height, cfg.fps, cfg.codec,
                                cfg.bitrate as f64 / 1_000_000.0,
                            ));
                            session_cfg = Some(cfg);
                            break;
                        }
                    }
                }
            }
            Ok(_) => {}
            Err(ref e) if is_timeout(e) => {
                // Повторяем punch
                let _ = socket.send_to(&kf_pkt, host_addr);
            }
            Err(e) => return EvrtConnectResult::Error(format!("EVRT recv: {e}")),
        }
    }

    let session_cfg = match session_cfg {
        Some(c) => c,
        None => return EvrtConnectResult::NoResponse,
    };

    let _ = events.send(SessionEvent::Info(format!(
        "EVRT прямое подключение: {}×{}@{} fps {} {:.1} Mбит/с",
        session_cfg.width, session_cfg.height, session_cfg.fps,
        session_cfg.codec,
        session_cfg.bitrate as f64 / 1_000_000.0,
    )));
    // ★ Уведомляем UI — EVRT активен
    let _ = events.send(SessionEvent::EvrtStatus {
        active:    true,
        host_addr: host_addr.ip().to_string(),
        port:      host_addr.port(),
    });

    // ── Внутреннее состояние для метрик ──────────────────────────────────────
    let last_arrival_us  = Arc::new(AtomicU64::new(0));
    let arrival_delta_ms = Arc::new(AtomicI32::new(-1));
    let decode_delta_ms  = Arc::new(AtomicI32::new(-1));
    let queued_units     = Arc::new(AtomicI32::new(0));
    let dropped_units    = Arc::new(AtomicU64::new(0));
    let decode_fps_atom  = Arc::new(AtomicI32::new(0));

    // ── Очередь кадров ────────────────────────────────────────────────────────
    let queue_cfg = if session_cfg.is_cinema_smooth() {
        FrameQueueConfig::cinema()
    } else {
        FrameQueueConfig::default() // игровой режим
    };
    let queue = Arc::new(FrameQueue::new(queue_cfg));

    // ── Декодер: поток берёт из queue и шлёт SessionEvent::Frame ─────────────
    let decode_queue   = queue.clone();
    let decode_events  = events.clone();
    let decode_stop    = stop.clone();
    let decode_delta_c = decode_delta_ms.clone();
    let decode_fps_c   = decode_fps_atom.clone();
    let queued_c       = queued_units.clone();
    let dropped_c      = dropped_units.clone();
    let cfg_codec      = session_cfg.codec.clone();
    let cfg_w          = session_cfg.width;
    let cfg_h          = session_cfg.height;

    let decode_handle = thread::spawn(move || {
        evrt_decode_loop(
            decode_queue,
            decode_events,
            decode_stop,
            cfg_codec,
            cfg_w,
            cfg_h,
            decode_delta_c,
            decode_fps_c,
            queued_c,
            dropped_c,
        );
    });

    // ── Поток приёма UDP → reassembler → queue ────────────────────────────────
    let recv_socket     = socket.clone();
    let recv_stop       = stop.clone();
    let recv_queue      = queue.handle();
    let recv_events     = events.clone();
    let recv_arrival    = last_arrival_us.clone();
    let recv_delta      = arrival_delta_ms.clone();

    // Аудио-плеер (shared между receive loop и audio thread)
    let audio_player = Arc::new(std::sync::Mutex::new(
        crate::evrt_audio::AudioPlayer::new()
    ));

    let recv_handle = thread::spawn(move || {
        let mut reassembler       = ChannelReassembler::new();
        let mut audio_re          = crate::evrt_audio::AudioReassembler::new();
        let mut buf               = vec![0u8; evrt::MAX_PACKET_SIZE + 64];
        let mut last_pkt_at       = Instant::now();
        let audio_player_recv     = audio_player.clone();
        recv_socket.set_read_timeout(Some(Duration::from_millis(500))).ok();

        while !recv_stop.load(Ordering::Relaxed) {
            match recv_socket.recv_from(&mut buf) {
                Ok((len, src)) if src == host_addr => {
                    let now_us = evrt::now_us();
                    let prev   = recv_arrival.swap(now_us, Ordering::Relaxed);
                    if prev > 0 {
                        let delta = ((now_us.saturating_sub(prev)) / 1000) as i32;
                        recv_delta.store(delta, Ordering::Relaxed);
                    }
                    last_pkt_at = Instant::now();

                    if let Some(pkt) = evrt::parse(&buf, len) {
                        match pkt.packet_type {
                            evrt::TYPE_CODEC_CONFIG => {
                                reassembler.set_codec_config(pkt.payload.clone());
                            }
                            evrt::TYPE_VIDEO_FRAME => {
                                if let Some((bytes, key, _delay_ms, pts)) =
                                    reassembler.on_packet(&pkt)
                                {
                                    recv_queue.enqueue(bytes, key, pts);
                                }
                            }
                            evrt::TYPE_SESSION_CONFIG => {
                                if let Some(cfg) = SessionConfig::from_json(&pkt.payload) {
                                    evrt_log(&recv_events, format!(
                                        "EVRT: SessionConfig update {:.1}Mbps @{}fps",
                                        cfg.bitrate as f64 / 1_000_000.0,
                                        cfg.fps,
                                    ));
                                }
                            }
                            // ── Аудио ────────────────────────────────────────
                            evrt::TYPE_AUDIO_CONFIG => {
                                if let Some(audio_cfg) =
                                    crate::evrt_audio::AudioConfig::from_json(&pkt.payload)
                                {
                                    evrt_log(&recv_events, format!(
                                        "EVRT Audio: {}Hz {}ch {}bit",
                                        audio_cfg.sample_rate,
                                        audio_cfg.channels,
                                        audio_cfg.bits_per_sample,
                                    ));
                                    if let Ok(mut player) = audio_player_recv.lock() {
                                        player.init(&audio_cfg);
                                    }
                                }
                            }
                            evrt::TYPE_AUDIO_FRAME => {
                                if let Some(pcm) = audio_re.on_packet(&pkt) {
                                    if let Ok(mut player) = audio_player_recv.lock() {
                                        player.play(&pcm);
                                    }
                                }
                            }
                            // ── ROI — логируем для диагностики ───────────────
                            evrt::TYPE_ROI_METADATA => {
                                // ROI используется для оптимизации рендеринга.
                                // Сейчас просто принимаем — можно добавить
                                // подсветку изменённой области в UI.
                                let _ = evrt::RoiRect::from_json(&pkt.payload);
                            }
                            _ => {}
                        }
                    }
                }
                Ok(_) => {} // чужой пакет
                Err(ref e) if is_timeout(e) => {
                    if last_pkt_at.elapsed() > IDLE_TIMEOUT {
                        evrt_log(&recv_events, "EVRT: idle timeout".into());
                        recv_stop.store(true, Ordering::Relaxed);
                        break;
                    }
                }
                Err(e) => {
                    evrt_log(&recv_events, format!("EVRT recv error: {e}"));
                    recv_stop.store(true, Ordering::Relaxed);
                    break;
                }
            }
        }

        recv_queue.close();
    });

    // ── Feedback loop (этот поток) ────────────────────────────────────────────
    let feedback_interval = if ultra_low_latency {
        FEEDBACK_INTERVAL_ULL
    } else {
        FEEDBACK_INTERVAL_NORM
    };

    let mut jitter       = AdaptiveJitter::new();
    let mut last_fb_at   = Instant::now();
    let mut queue_drops_seen = 0u64;
    let cinema = session_cfg.is_cinema_smooth();

    socket.set_read_timeout(Some(Duration::from_millis(50))).ok();

    while !stop.load(Ordering::Relaxed) {
        thread::sleep(Duration::from_millis(20));

        if last_fb_at.elapsed() < feedback_interval {
            continue;
        }
        last_fb_at = Instant::now();

        // Собираем метрики
        let arr_delta   = arrival_delta_ms.load(Ordering::Relaxed);
        let dec_delta   = decode_delta_ms.load(Ordering::Relaxed);
        let queued      = queued_units.load(Ordering::Relaxed).max(0) as u32;
        let drops       = dropped_units.load(Ordering::Relaxed);
        let new_drops   = drops.saturating_sub(queue_drops_seen);
        queue_drops_seen = drops;
        let fps_decoded = decode_fps_atom.load(Ordering::Relaxed).max(0) as u32;

        // Вычислить pressure
        let pressure = compute_pressure(arr_delta, dec_delta, queued, new_drops, cinema);

        // Адаптивный jitter
        let jitter_ms = jitter.update(pressure, arr_delta, queued, new_drops, cinema);
        queue.set_jitter_delay(Duration::from_millis(jitter_ms as u64));

        let fb = ReceiverFeedback {
            pressure,
            backlog_frames:    queued,
            queue_drops:       drops,
            decode_fps:        fps_decoded,
            assembly_delay_ms: 0,
            arrival_delta_ms:  arr_delta,
            decode_delta_ms:   dec_delta,
            present_delta_ms:  -1,
            pulse_estimate_ms: -1,
            input_estimate_ms: -1,
        };

        let pkt = evrt::build_receiver_feedback(&fb);
        let _ = socket.send_to(&pkt, host_addr);

        // ★ Метрики → UI (каждый тик feedback loop)
        let _ = events.send(SessionEvent::EvrtMetrics {
            pressure:         pressure.as_str().to_owned(),
            arrival_delta_ms: arr_delta,
            decode_delta_ms:  dec_delta,
            jitter_ms,
            bitrate_mbps:     0.0, // хост сообщает в SessionConfig keepalive
            fps:              fps_decoded,
        });

        // Если critical + задержка растёт → запрос keyframe
        if pressure == Pressure::Critical && queued > 0 {
            let kf = evrt::build_request_key_frame();
            let _ = socket.send_to(&kf, host_addr);
            queue.wait_for_keyframe();
        }
    }

    queue.close();
    let _ = recv_handle.join();
    let _ = decode_handle.join();

    // ★ Уведомляем UI — EVRT завершён
    let _ = events.send(SessionEvent::EvrtStatus {
        active:    false,
        host_addr: host_addr.ip().to_string(),
        port:      host_addr.port(),
    });

    evrt_log(&events, "EVRT client session ended".into());
    EvrtConnectResult::Ok
}

// ─── декодер-петля ─────────────────────────────────────────────────────────────

fn evrt_decode_loop(
    queue:      Arc<FrameQueue>,
    events:     Sender<SessionEvent>,
    stop:       Arc<AtomicBool>,
    codec:      String,
    width:      u32,
    height:     u32,
    delta_ms:   Arc<AtomicI32>,
    fps_atom:   Arc<AtomicI32>,
    queued:     Arc<AtomicI32>,
    dropped:    Arc<AtomicU64>,
) {
    // ── Инициализация декодеров (те же что в decode_frame_loop) ──────────────
    #[cfg(feature = "live-h264")]
    let mut h264_sw = openh264::decoder::Decoder::new().ok();
    #[cfg(not(feature = "live-h264"))]
    let mut h264_sw: Option<()> = None;

    let mut h264_vt = if crate::videotoolbox::videotoolbox_h264_decoder_available() {
        Some(crate::videotoolbox::VideoToolboxH264Decoder::new())
    } else {
        None
    };
    let mut vt_fail_streak = 0u32;

    let mut h265_mf: Option<crate::mf_video::MfVideoDecoder> = None;
    let mf_status = crate::mf_video::mf_video_decode_status();

    // FPS-счётчик
    let mut decoded_count = 0u64;
    let mut fps_window_start = Instant::now();

    let mut last_decode_at = Instant::now();

    loop {
        // Обновить статистику очереди
        let stats = queue.stats();
        queued.store(stats.queued_units as i32, Ordering::Relaxed);
        dropped.store(stats.dropped_units, Ordering::Relaxed);

        // Взять кадр из очереди
        let Some((bytes, _is_key, _pts)) = queue.dequeue(&stop) else {
            break;
        };

        let decode_start = Instant::now();

        // delta между декодами
        let now = Instant::now();
        let d = now.duration_since(last_decode_at).as_millis() as i32;
        delta_ms.store(d, Ordering::Relaxed);
        last_decode_at = now;

        // Декодировать в зависимости от кодека
        let maybe_event = match codec.to_ascii_uppercase().as_str() {
            "H264" => decode_h264_frame(
                &bytes, width, height,
                &mut h264_vt, &mut h264_sw, &mut vt_fail_streak,
            ),
            "H265" | "HEVC" => decode_h265_frame(
                &bytes, width, height, &mut h265_mf, mf_status.h265,
            ),
            _ => decode_h264_frame(
                &bytes, width, height,
                &mut h264_vt, &mut h264_sw, &mut vt_fail_streak,
            ),
        };

        // FPS-счётчик
        decoded_count += 1;
        let fps_elapsed = fps_window_start.elapsed();
        if fps_elapsed >= Duration::from_secs(1) {
            let fps = (decoded_count as f64 / fps_elapsed.as_secs_f64()) as i32;
            fps_atom.store(fps, Ordering::Relaxed);
            decoded_count = 0;
            fps_window_start = Instant::now();
        }

        let decode_ms = decode_start.elapsed().as_millis() as u64;

        if let Some((rgba, w, h)) = maybe_event {
            let _ = events.send(SessionEvent::FrameMetrics {
                bytes: bytes.len(),
                queue_ms: 0,
                decode_ms,
                dropped: 0,
            });
            let sid = format!("evrt-{decoded_count}");
            let _ = events.send(SessionEvent::Frame {
                sid,
                codec: codec.clone(),
                width:  w,
                height: h,
                rgba,
            });
        }
    }

    evrt_log(&events, "EVRT decode loop exited".into());
}

// ─── декодирование H264 ────────────────────────────────────────────────────────

fn decode_h264_frame(
    bytes:        &[u8],
    _width:       u32,
    _height:      u32,
    vt:           &mut Option<crate::videotoolbox::VideoToolboxH264Decoder>,
    #[cfg(feature = "live-h264")]
    sw:           &mut Option<openh264::decoder::Decoder>,
    #[cfg(not(feature = "live-h264"))]
    _sw:          &mut Option<()>,
    vt_failures:  &mut u32,
) -> Option<(Vec<u8>, usize, usize)> {
    const VT_FAIL_LIMIT: u32 = 5;

    // macOS VideoToolbox — API: decode_packets(iter) → (w, h, rgba)
    if let Some(ref mut dec) = vt {
        if *vt_failures < VT_FAIL_LIMIT {
            match dec.decode_packets(std::iter::once(bytes.to_vec())) {
                Ok(Some((w, h, rgba))) => {
                    *vt_failures = 0;
                    return Some((rgba, w, h));
                }
                Ok(None) => { *vt_failures += 1; }
                Err(_)   => { *vt_failures += 1; }
            }
        }
    }

    // OpenH264 software fallback — нужен трейт YUVSource для .dimensions()
    #[cfg(feature = "live-h264")]
    if let Some(ref mut dec) = sw {
        use openh264::formats::YUVSource;
        if let Ok(Some(yuv)) = dec.decode(bytes) {
            let (w, h) = yuv.dimensions();
            if w > 0 && h > 0 {
                let mut rgba = vec![0u8; w * h * 4];
                yuv.write_rgba8(&mut rgba);
                return Some((rgba, w, h));
            }
        }
    }

    None
}

// ─── декодирование H265 ────────────────────────────────────────────────────────

fn decode_h265_frame(
    bytes:    &[u8],
    width:    u32,
    height:   u32,
    mf_dec:   &mut Option<crate::mf_video::MfVideoDecoder>,
    mf_avail: bool,
) -> Option<(Vec<u8>, usize, usize)> {
    use crate::mf_video::MfVideoCodec;

    if !mf_avail { return None; }

    let dec = mf_dec.get_or_insert_with(|| {
        crate::mf_video::MfVideoDecoder::new(MfVideoCodec::H265, width, height)
            .expect("H265 MF decoder init")
    });

    match dec.decode_packets(std::iter::once(bytes.to_vec())) {
        Ok(Some((w, h, rgba))) => Some((rgba, w, h)),
        _ => None,
    }
}

// ─── pressure ─────────────────────────────────────────────────────────────────

fn compute_pressure(
    arrival_delta_ms: i32,
    decode_delta_ms:  i32,
    backlog:          u32,
    new_drops:        u64,
    cinema:           bool,
) -> Pressure {
    let (high_ms, crit_ms, backlog_crit, backlog_high) = if cinema {
        (30, 44, 3, 2)
    } else {
        (22, 34, 2, 1)
    };

    let crit = arrival_delta_ms >= crit_ms
        || decode_delta_ms >= crit_ms
        || backlog >= backlog_crit
        || new_drops >= 3;

    let high = crit
        || arrival_delta_ms >= high_ms
        || decode_delta_ms >= high_ms
        || backlog >= backlog_high
        || new_drops >= 1;

    if crit  { Pressure::Critical }
    else if high { Pressure::High }
    else    { Pressure::Normal }
}

// ─── вспомогательные ──────────────────────────────────────────────────────────

fn is_timeout(e: &std::io::Error) -> bool {
    e.kind() == std::io::ErrorKind::WouldBlock
        || e.kind() == std::io::ErrorKind::TimedOut
}

fn evrt_log(events: &Sender<SessionEvent>, msg: String) {
    eprintln!("[evrt-client] {msg}");
    let _ = events.send(SessionEvent::Info(msg));
}

// ─── интеграционный хелпер: пробовать EVRT перед TCP relay ───────────────────

/// Попробовать установить прямое EVRT-соединение.
/// Если хост не отвечает за `CONNECT_TIMEOUT` — вернуть `None` (нужен relay).
pub fn try_evrt_before_relay(
    local_udp:     &Arc<UdpSocket>,
    host_addr:     SocketAddr,
    events:        &Sender<SessionEvent>,
    stop:          Arc<AtomicBool>,
    ultra_low_lat: bool,
) -> bool {
    let params = EvrtClientParams {
        socket:           local_udp.clone(),
        host_addr,
        events:           events.clone(),
        stop,
        ultra_low_latency: ultra_low_lat,
    };

    match run_evrt_client(params) {
        EvrtConnectResult::Ok => true,
        EvrtConnectResult::NoResponse => {
            evrt_log(events, "EVRT: no response — falling back to TCP relay".into());
            false
        }
        EvrtConnectResult::Error(e) => {
            evrt_log(events, format!("EVRT error ({e}) — falling back to TCP relay"));
            false
        }
    }
}

// ─── тесты ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pressure_normal_on_clean_stream() {
        let p = compute_pressure(5, 3, 0, 0, false);
        assert_eq!(p, Pressure::Normal);
    }

    #[test]
    fn pressure_high_on_backlog() {
        let p = compute_pressure(10, 10, 1, 0, false);
        assert_eq!(p, Pressure::High);
    }

    #[test]
    fn pressure_critical_on_drops() {
        let p = compute_pressure(40, 40, 3, 5, false);
        assert_eq!(p, Pressure::Critical);
    }

    #[test]
    fn cinema_mode_higher_thresholds() {
        // В game-режиме: delta=25 >= high_ms=22 → High
        let p_game = compute_pressure(25, 25, 0, 0, false);
        assert_eq!(p_game, Pressure::High);

        // В cinema-режиме: delta=25 < high_ms=30, backlog=0 → Normal
        let p_cinema = compute_pressure(25, 25, 0, 0, true);
        assert_eq!(p_cinema, Pressure::Normal);

        // В cinema-режиме: delta=35 >= high_ms=30 → High (но < crit_ms=44)
        let p_cinema_high = compute_pressure(35, 35, 0, 0, true);
        assert_eq!(p_cinema_high, Pressure::High);
    }
}
