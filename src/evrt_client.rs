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
    evrt::{self, Pressure, ReceiverFeedback, SessionConfig},
    frame_queue::{AdaptiveJitter, ChannelReassembler, FrameQueue, FrameQueueConfig},
    transport::SessionEvent,
};
use socket2::SockRef;

// ─── глобальное состояние клиента ─────────────────────────────────────────────

/// Максимальное разрешение экрана клиента, упакованное как (w << 32 | h).
/// 0 = не задано. Устанавливается JNI-вызовом setMaxResolution перед сессией.
static CLIENT_MAX_RES: AtomicU64 = AtomicU64::new(0);

/// Вызывается из JNI при старте: сообщает хосту максимальное разрешение экрана.
pub fn set_max_resolution(w: u32, h: u32) {
    CLIENT_MAX_RES.store(((w as u64) << 32) | (h as u64), Ordering::Relaxed);
}

/// Read back whatever `set_max_resolution` last stored — `(0, 0)` if never
/// called. Live-found (chasing EVRT2's fps gap vs this file's own EVRT1
/// pipeline): `MainActivity.kt` already calls `setMaxResolution` with the
/// real device screen size (`dm.widthPixels`/`heightPixels`) before any
/// connection, EVRT1 or EVRT2 — but the EVRT2 experiment's own client HELLO
/// (`evrt2_experiment.rs`) never read it back, sending a hardcoded
/// `(1920, 1080)` instead of the phone's actual, usually much smaller,
/// real width. Exposed here so that HELLO can reuse the SAME value EVRT1
/// already has, instead of duplicating the JNI plumbing for a second
/// client-side resolution hint.
pub fn max_resolution() -> (u32, u32) {
    let packed = CLIENT_MAX_RES.load(Ordering::Relaxed);
    (
        ((packed >> 32) & 0xFFFF_FFFF) as u32,
        (packed & 0xFFFF_FFFF) as u32,
    )
}

// ─── константы ────────────────────────────────────────────────────────────────

const CONNECT_TIMEOUT: Duration = Duration::from_secs(4);
const IDLE_TIMEOUT: Duration = Duration::from_secs(6);
const FEEDBACK_INTERVAL_ULL: Duration = Duration::from_millis(70);
const FEEDBACK_INTERVAL_NORM: Duration = Duration::from_millis(150);
const PUNCH_REPEATS: usize = 3;
const PUNCH_GAP: Duration = Duration::from_millis(30);

// ─── Android EVRT liveness flag ────────────────────────────────────────────────
// Android's direct-to-Surface decode path (decode_h264/h265/av1_frame below)
// renders via MediaCodec and returns `None` — no `SessionEvent::Frame` is ever
// sent for those frames. `transport::run_session`'s "have we seen live video"
// watchdog only learns about frames from `SessionEvent::Frame`, so on Android
// it never saw EVRT deliver anything and kept resending SetClientCodec every
// 3s for the whole session — forcing a full host-side IDR each time even
// during smooth 60fps playback. This flag closes that gap.
static ANDROID_EVRT_FRAME_SEEN: AtomicBool = AtomicBool::new(false);

/// Has the Android direct-to-Surface path decoded at least one EVRT frame in
/// the current session? Sticky — stays true once set; call
/// `reset_android_evrt_frame_seen` at the start of a new session.
pub fn android_evrt_frame_seen() -> bool {
    ANDROID_EVRT_FRAME_SEEN.load(Ordering::Relaxed)
}

/// Reset at the start of each `run_session` so a previous session's frames
/// don't mask a genuine "no video yet" state in a new one.
pub fn reset_android_evrt_frame_seen() {
    ANDROID_EVRT_FRAME_SEEN.store(false, Ordering::Relaxed);
}

// ─── публичный интерфейс ──────────────────────────────────────────────────────

/// Параметры EVRT-клиента.
pub struct EvrtClientParams {
    /// UDP-сокет клиента (уже забиндированный на локальном порту).
    pub socket: Arc<UdpSocket>,
    /// Внешний адрес хоста из rendezvous ответа.
    pub host_addr: SocketAddr,
    /// Канал событий → UI.
    pub events: Sender<SessionEvent>,
    /// Сигнал остановки.
    pub stop: Arc<AtomicBool>,
    /// Ultra-low-latency режим (feedback каждые 70мс вместо 150мс).
    pub ultra_low_latency: bool,
    pub session_token: Option<String>,
    /// Per-viewer playback state; changing it must not affect other sessions.
    pub audio_enabled: Arc<AtomicBool>,
    /// How long to wait for the initial SessionConfig from this candidate.
    pub connect_timeout: Duration,
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
    let EvrtClientParams {
        socket,
        host_addr,
        events,
        stop,
        ultra_low_latency,
        session_token,
        audio_enabled,
        connect_timeout,
    } = params;

    evrt_log(&events, format!("EVRT client: punching to {host_addr}"));
    let socket_ref = SockRef::from(socket.as_ref());
    let _ = socket_ref.set_recv_buffer_size(4 * 1024 * 1024);
    let _ = socket_ref.set_send_buffer_size(512 * 1024);
    if let Ok(size) = socket_ref.recv_buffer_size() {
        evrt_log(
            &events,
            format!("EVRT client UDP receive buffer: {size} bytes"),
        );
    }

    // ── UDP punch-hole ────────────────────────────────────────────────────────
    for _ in 0..PUNCH_REPEATS {
        let _ = socket.send_to(&[0u8], host_addr);
        thread::sleep(PUNCH_GAP);
    }

    // Отправляем RequestKeyFrame — хост по нему определяет что клиент живой
    let kf_pkt = evrt::build_request_key_frame_authenticated(session_token.as_deref());
    let _ = socket.send_to(&kf_pkt, host_addr);

    // ── Ожидаем SessionConfig от хоста ───────────────────────────────────────
    socket
        .set_read_timeout(Some(Duration::from_millis(300)))
        .ok();
    let mut buf = vec![0u8; evrt::MAX_PACKET_SIZE + 64];
    let deadline = Instant::now() + connect_timeout;
    let mut session_cfg: Option<SessionConfig> = None;

    while Instant::now() < deadline && !stop.load(Ordering::Relaxed) {
        match socket.recv_from(&mut buf) {
            Ok((len, src)) if src == host_addr => {
                if let Some(pkt) = evrt::parse_authenticated(&buf, len, session_token.as_deref()) {
                    if pkt.packet_type == evrt::TYPE_SESSION_CONFIG {
                        if let Some(cfg) = SessionConfig::from_json(&pkt.payload) {
                            evrt_log(
                                &events,
                                format!(
                                    "EVRT: SessionConfig received — {}x{}@{} {} {:.1}Mbps",
                                    cfg.width,
                                    cfg.height,
                                    cfg.fps,
                                    cfg.codec,
                                    cfg.bitrate as f64 / 1_000_000.0,
                                ),
                            );
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
        session_cfg.width,
        session_cfg.height,
        session_cfg.fps,
        session_cfg.codec,
        session_cfg.bitrate as f64 / 1_000_000.0,
    )));
    // ★ Уведомляем UI — EVRT активен
    let _ = events.send(SessionEvent::EvrtStatus {
        active: true,
        host_addr: host_addr.ip().to_string(),
        port: host_addr.port(),
    });

    // ── Внутреннее состояние для метрик ──────────────────────────────────────
    let last_arrival_us = Arc::new(AtomicU64::new(0));
    let arrival_delta_ms = Arc::new(AtomicI32::new(-1));
    let decode_delta_ms = Arc::new(AtomicI32::new(-1));
    let decoded_frames = Arc::new(AtomicU64::new(0));
    let packets_received = Arc::new(AtomicU64::new(0));
    let frames_assembled = Arc::new(AtomicU64::new(0));
    let reassembly_drops = Arc::new(AtomicU64::new(0));
    let assembly_delay_ms = Arc::new(AtomicI32::new(-1));
    let configured_bitrate = Arc::new(AtomicU64::new(session_cfg.bitrate as u64));

    // ── Очередь кадров ────────────────────────────────────────────────────────
    let queue_cfg = if session_cfg.is_cinema_smooth() {
        FrameQueueConfig::cinema()
    } else {
        FrameQueueConfig::default() // игровой режим
    };
    let queue = Arc::new(FrameQueue::new(queue_cfg));

    // ── Декодер: поток берёт из queue и шлёт SessionEvent::Frame ─────────────
    let decode_queue = queue.clone();
    let decode_events = events.clone();
    let decode_stop = stop.clone();
    let decode_delta_c = decode_delta_ms.clone();
    let decoded_frames_c = decoded_frames.clone();
    let cfg_codec = session_cfg.codec.clone();
    let cfg_w = session_cfg.width;
    let cfg_h = session_cfg.height;

    // Канал для динамической смены кодека: receive_loop → decode_loop.
    // Хост посылает TYPE_CODEC_CONFIG с ASCII именем кодека когда меняет энкодер.
    let (codec_change_tx, codec_change_rx) = std::sync::mpsc::channel::<String>();

    let decode_handle = thread::spawn(move || {
        evrt_decode_loop(
            decode_queue,
            decode_events,
            decode_stop,
            cfg_codec,
            codec_change_rx,
            cfg_w,
            cfg_h,
            decode_delta_c,
            decoded_frames_c,
        );
    });

    // ── Поток приёма UDP → reassembler → queue ────────────────────────────────
    let recv_socket = socket.clone();
    let recv_stop = stop.clone();
    let recv_queue = queue.handle();
    let recv_events = events.clone();
    let recv_arrival = last_arrival_us.clone();
    let recv_delta = arrival_delta_ms.clone();
    let recv_packets = packets_received.clone();
    let recv_assembled = frames_assembled.clone();
    let recv_reassembly_drops = reassembly_drops.clone();
    let recv_assembly_delay = assembly_delay_ms.clone();
    let recv_bitrate = configured_bitrate.clone();
    let recv_session_token = session_token.clone();

    // WASAPI playback lives inside the receive thread: COM objects stay
    // on the same thread where they are created and used.
    let recv_handle = thread::spawn(move || {
        let mut reassembler = ChannelReassembler::new();
        let mut audio_re = crate::evrt_audio::AudioReassembler::new();
        let mut audio_player = crate::evrt_audio::AudioPlayer::new();
        let mut audio_was_enabled = true;
        let mut buf = vec![0u8; evrt::MAX_PACKET_SIZE + 64];
        let mut last_pkt_at = Instant::now();
        let mut last_loss_keyframe_request = Instant::now() - Duration::from_secs(1);
        recv_socket
            .set_read_timeout(Some(Duration::from_millis(10)))
            .ok();

        while !recv_stop.load(Ordering::Relaxed) {
            let audio_is_enabled = audio_enabled.load(Ordering::Acquire);
            if audio_was_enabled && !audio_is_enabled {
                audio_player.clear_buffer();
            }
            audio_was_enabled = audio_is_enabled;
            if audio_is_enabled {
                audio_player.tick();
            }
            match recv_socket.recv_from(&mut buf) {
                Ok((len, src)) if src == host_addr => {
                    let now_us = evrt::now_us();
                    let prev = recv_arrival.swap(now_us, Ordering::Relaxed);
                    if prev > 0 {
                        let delta = ((now_us.saturating_sub(prev)) / 1000) as i32;
                        recv_delta.store(delta, Ordering::Relaxed);
                    }
                    last_pkt_at = Instant::now();

                    if let Some(pkt) =
                        evrt::parse_authenticated(&buf, len, recv_session_token.as_deref())
                    {
                        recv_packets.fetch_add(1, Ordering::Relaxed);
                        match pkt.packet_type {
                            evrt::TYPE_CODEC_CONFIG => {
                                // Отличаем SPS/PPS (начинается с 0x00 — NAL start code)
                                // от имени кодека (ASCII ≥ 0x41).
                                // Хост шлёт имя кодека при смене: "H264", "H265", "EVRTCK", "AV1".
                                if pkt.payload.first().copied().unwrap_or(0) >= 0x41 {
                                    if let Ok(name) = std::str::from_utf8(&pkt.payload) {
                                        let upper = name.to_ascii_uppercase();
                                        let _ = codec_change_tx.send(upper);
                                    }
                                } else {
                                    reassembler.set_codec_config(pkt.payload.clone());
                                }
                            }
                            evrt::TYPE_FEC => {
                                // FEC-пакет: может восстановить потерянный data-пакет
                                // и завершить кадр без ожидания IDR.
                                if let Some((bytes, key, delay_ms, pts)) =
                                    reassembler.on_fec_packet(&pkt)
                                {
                                    recv_assembly_delay.store(delay_ms, Ordering::Relaxed);
                                    recv_assembled.fetch_add(1, Ordering::Relaxed);
                                    recv_queue.enqueue(bytes, key, pts);
                                }
                            }
                            evrt::TYPE_VIDEO_FRAME => {
                                let drops_before = reassembler.dropped_frames();
                                if let Some((bytes, key, delay_ms, pts)) =
                                    reassembler.on_packet(&pkt)
                                {
                                    recv_assembly_delay.store(delay_ms, Ordering::Relaxed);
                                    recv_assembled.fetch_add(1, Ordering::Relaxed);
                                    recv_queue.enqueue(bytes, key, pts);
                                }
                                let drops_after = reassembler.dropped_frames();
                                recv_reassembly_drops.store(drops_after, Ordering::Relaxed);
                                if drops_after > drops_before
                                    && last_loss_keyframe_request.elapsed()
                                        >= Duration::from_millis(250)
                                {
                                    let _ = recv_socket.send_to(
                                        &evrt::build_request_key_frame_authenticated(
                                            recv_session_token.as_deref(),
                                        ),
                                        host_addr,
                                    );
                                    // On Android Surface path MediaCodec handles incomplete frames
                                    // gracefully (brief artifact vs multi-second freeze on WiFi loss).
                                    #[cfg(not(all(
                                        target_os = "android",
                                        feature = "android-client"
                                    )))]
                                    recv_queue.wait_for_keyframe();
                                    last_loss_keyframe_request = Instant::now();
                                }
                            }
                            evrt::TYPE_SESSION_CONFIG => {
                                if let Some(cfg) = SessionConfig::from_json(&pkt.payload) {
                                    recv_bitrate.store(cfg.bitrate as u64, Ordering::Relaxed);
                                    evrt_log(
                                        &recv_events,
                                        format!(
                                            "EVRT: SessionConfig update {:.1}Mbps @{}fps",
                                            cfg.bitrate as f64 / 1_000_000.0,
                                            cfg.fps,
                                        ),
                                    );
                                }
                            }
                            // ── Аудио ────────────────────────────────────────
                            evrt::TYPE_AUDIO_CONFIG => {
                                if let Some(audio_cfg) =
                                    crate::evrt_audio::AudioConfig::from_json(&pkt.payload)
                                {
                                    evrt_log(
                                        &recv_events,
                                        format!(
                                            "EVRT Audio: {}Hz {}ch {}bit",
                                            audio_cfg.sample_rate,
                                            audio_cfg.channels,
                                            audio_cfg.bits_per_sample,
                                        ),
                                    );
                                    crate::evrt_audio::set_audio_sample_rate(audio_cfg.sample_rate);
                                    audio_player.init(&audio_cfg);
                                }
                            }
                            evrt::TYPE_AUDIO_FRAME => {
                                if audio_is_enabled {
                                    if let Some(pcm) = audio_re.on_packet(&pkt) {
                                        audio_player.play(&pcm);
                                    }
                                } else {
                                    // Continue feeding the reassembler so a later unmute starts
                                    // from a complete frame without retaining muted PCM.
                                    let _ = audio_re.on_packet(&pkt);
                                }
                            }
                            // ── ROI — логируем для диагностики ───────────────
                            evrt::TYPE_ROI_METADATA => {
                                // ROI используется для оптимизации рендеринга.
                                // Сейчас просто принимаем — можно добавить
                                // подсветку изменённой области в UI.
                                let _ = evrt::RoiRect::from_bytes(&pkt.payload);
                            }
                            _ => {}
                        }
                    }
                }
                Ok(_) => {} // чужой пакет
                Err(ref e) if is_timeout(e) => {
                    if audio_is_enabled {
                        audio_player.tick();
                    }
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

    let mut jitter = AdaptiveJitter::new();
    let mut last_fb_at = Instant::now();
    let mut drops_seen = 0u64;
    let mut last_fps_at = Instant::now();
    let mut last_decoded_frames = 0_u64;
    let mut fps_decoded = 0_u32;
    let cinema = session_cfg.is_cinema_smooth();

    socket
        .set_read_timeout(Some(Duration::from_millis(50)))
        .ok();

    while !stop.load(Ordering::Relaxed) {
        thread::sleep(Duration::from_millis(20));

        if last_fb_at.elapsed() < feedback_interval {
            continue;
        }
        last_fb_at = Instant::now();

        // Собираем метрики
        let arr_delta = arrival_delta_ms.load(Ordering::Relaxed);
        let assembly_delay = assembly_delay_ms.load(Ordering::Relaxed);

        // На Android Surface: реальные decode-метрики из PerfStats через JNI (один вызов).
        // На остальных платформах: из atomic counters decode loop.
        #[cfg(all(target_os = "android", feature = "android-client"))]
        let (dec_delta, android_total_decoded) = {
            let (total, ms) = crate::android_video::get_android_decode_stats();
            (ms, total)
        };
        #[cfg(not(all(target_os = "android", feature = "android-client")))]
        let dec_delta = decode_delta_ms.load(Ordering::Relaxed);

        let queue_stats = queue.stats();
        let queued = queue_stats.queued_units as u32;
        let queue_drops = queue_stats.dropped_units;
        let assembly_drops = reassembly_drops.load(Ordering::Relaxed);
        let drops = queue_drops.saturating_add(assembly_drops);
        let new_drops = drops.saturating_sub(drops_seen);
        drops_seen = drops;
        if last_fps_at.elapsed() >= Duration::from_secs(1) {
            #[cfg(all(target_os = "android", feature = "android-client"))]
            let decoded = android_total_decoded;
            #[cfg(not(all(target_os = "android", feature = "android-client")))]
            let decoded = decoded_frames.load(Ordering::Relaxed);

            let delta = decoded.saturating_sub(last_decoded_frames);
            // Keep last non-zero fps — a zero window means decoder stalled, not 0fps.
            if delta > 0 {
                fps_decoded = (delta as f64 / last_fps_at.elapsed().as_secs_f64()).round() as u32;
            }
            last_decoded_frames = decoded;
            last_fps_at = Instant::now();
        }

        // Вычислить pressure
        let pressure = compute_pressure(
            arr_delta,
            dec_delta,
            queued,
            new_drops,
            cinema,
            cfg!(all(target_os = "android", feature = "android-client")),
        );

        // Адаптивный jitter
        let jitter_ms = jitter.update(pressure, arr_delta, queued, new_drops, cinema);
        queue.set_jitter_delay(Duration::from_millis(jitter_ms as u64));

        let packed_res = CLIENT_MAX_RES.load(Ordering::Relaxed);
        let fb = ReceiverFeedback {
            pressure,
            backlog_frames: queued,
            queue_drops: drops,
            decode_fps: fps_decoded,
            assembly_delay_ms: assembly_delay,
            arrival_delta_ms: arr_delta,
            decode_delta_ms: dec_delta,
            present_delta_ms: -1,
            pulse_estimate_ms: -1,
            input_estimate_ms: -1,
            max_width: (packed_res >> 32) as u32,
            max_height: (packed_res & 0xFFFF_FFFF) as u32,
        };

        let pkt = evrt::build_receiver_feedback_authenticated(&fb, session_token.as_deref());
        let _ = socket.send_to(&pkt, host_addr);

        // ★ Метрики → UI (каждый тик feedback loop)
        let _ = events.send(SessionEvent::EvrtMetrics {
            pressure: pressure.as_str().to_owned(),
            arrival_delta_ms: arr_delta,
            assembly_delay_ms: assembly_delay,
            decode_delta_ms: dec_delta,
            jitter_ms,
            bitrate_mbps: configured_bitrate.load(Ordering::Relaxed) as f32 / 1_000_000.0,
            fps: fps_decoded,
            packets_received: packets_received.load(Ordering::Relaxed),
            frames_assembled: frames_assembled.load(Ordering::Relaxed),
            reassembly_drops: assembly_drops,
            queue_drops,
        });

        // Если critical → запрос keyframe + заморозка очереди до нового IDR.
        // На Android Surface: MediaCodec обрабатывает IDR без паузы, и RequestKeyFrame
        // заставляет хоста ставить waiting_for_idr=true (33мс заморозка P-фреймов)
        // — это ухудшает ситуацию при медленном decode. Не посылаем kf на Android.
        #[cfg(not(all(target_os = "android", feature = "android-client")))]
        if pressure == Pressure::Critical && queued > 0 {
            let kf = evrt::build_request_key_frame_authenticated(session_token.as_deref());
            let _ = socket.send_to(&kf, host_addr);
            queue.wait_for_keyframe();
        }
    }

    queue.close();
    let _ = recv_handle.join();
    let _ = decode_handle.join();

    // ★ Уведомляем UI — EVRT завершён
    let _ = events.send(SessionEvent::EvrtStatus {
        active: false,
        host_addr: host_addr.ip().to_string(),
        port: host_addr.port(),
    });

    evrt_log(&events, "EVRT client session ended".into());
    EvrtConnectResult::Ok
}

// ─── декодер-петля ─────────────────────────────────────────────────────────────

fn evrt_decode_loop(
    queue: Arc<FrameQueue>,
    events: Sender<SessionEvent>,
    stop: Arc<AtomicBool>,
    mut codec: String,
    codec_change_rx: std::sync::mpsc::Receiver<String>,
    width: u32,
    height: u32,
    delta_ms: Arc<AtomicI32>,
    decoded_frames: Arc<AtomicU64>,
) {
    // ── Инициализация декодеров ───────────────────────────────────────────────
    let mut evrtck_dec = crate::evrtck::EvrtckDecoder::new();

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

    // macOS: hardware HEVC decode via VideoToolbox. Same availability gate as
    // the H264 VT decoder (true only on macOS).
    let mut h265_vt = if crate::videotoolbox::videotoolbox_h264_decoder_available() {
        Some(crate::videotoolbox::VideoToolboxH264Decoder::new_hevc())
    } else {
        None
    };
    let mut h265_vt_fail = 0u32;

    // VP9 через Windows Media Foundation (Win10 1803+).
    let mut vp9_mf = crate::vp9_mf::Vp9MfDecoder::new();

    // AV1 через Windows Media Foundation.
    let mut av1_mf: Option<crate::mf_video::MfVideoDecoder> = None;

    evrt_log(
        &events,
        format!("evrt_decode_loop: codec={codec} {width}x{height}"),
    );
    let mut diag_frames = 0u32;

    loop {
        // Обновить кодек если хост объявил смену
        while let Ok(new_codec) = codec_change_rx.try_recv() {
            if new_codec != codec {
                evrt_log(
                    &events,
                    format!("EVRT decode: codec {} → {}", codec, new_codec),
                );
                codec = new_codec;
            }
        }

        // Взять кадр из очереди
        let Some((bytes, is_key, _pts)) = queue.dequeue(&stop) else {
            break;
        };

        let decode_start = Instant::now();

        // Декодировать в зависимости от кодека
        let maybe_event = match codec.to_ascii_uppercase().as_str() {
            "EVRTCK" => {
                // Parse w/h from wire header before decode to avoid borrow conflict.
                // Wire: magic(4)+ver(1)+flags(1)+frame_id(4)+w(4)+h(4) → offsets 10..18
                let (ew, eh) = if bytes.len() >= 18 {
                    (
                        u32::from_le_bytes(bytes[10..14].try_into().unwrap()) as usize,
                        u32::from_le_bytes(bytes[14..18].try_into().unwrap()) as usize,
                    )
                } else {
                    (0, 0)
                };
                let flags = bytes.get(5).copied().unwrap_or(0);
                if diag_frames < 5 {
                    evrt_log(
                        &events,
                        format!(
                        "EVRTCK frame#{diag_frames}: len={} magic={:?} flags={flags} w={ew} h={eh}",
                        bytes.len(),
                        bytes.get(0..4).map(|b| b.to_vec()).unwrap_or_default(),
                    ),
                    );
                    diag_frames += 1;
                }
                // FLAG_NOP (0x02): screen unchanged — skip render, decoder state needs no update.
                if flags & crate::evrtck::FLAG_NOP != 0 {
                    None
                } else {
                    match evrtck_dec.decode_wire(&bytes) {
                        Ok(rgba) => Some((rgba.to_vec(), ew, eh)),
                        Err(e) => {
                            evrt_log(
                                &events,
                                format!(
                                    "EVRTCK decode error: {e} magic={:?}",
                                    bytes.get(0..4).map(|b| b.to_vec()).unwrap_or_default()
                                ),
                            );
                            None
                        }
                    }
                }
            }
            "H264" => decode_h264_frame(
                &bytes,
                width,
                height,
                is_key,
                &mut h264_vt,
                &mut h264_sw,
                &mut vt_fail_streak,
            ),
            "H265" | "HEVC" => decode_h265_frame(
                &bytes,
                width,
                height,
                is_key,
                &mut h265_vt,
                &mut h265_vt_fail,
                &mut h265_mf,
                mf_status.h265,
            ),
            "VP9" => decode_vp9_frame(&bytes, &mut vp9_mf, &events),
            "AV1" => decode_av1_frame(&bytes, width, height, is_key, &mut av1_mf, &events),
            _ => decode_h264_frame(
                &bytes,
                width,
                height,
                is_key,
                &mut h264_vt,
                &mut h264_sw,
                &mut vt_fail_streak,
            ),
        };

        let decode_ms = decode_start.elapsed().as_millis() as u64;
        delta_ms.store(decode_ms.min(i32::MAX as u64) as i32, Ordering::Relaxed);

        if let Some((rgba, w, h)) = maybe_event {
            let decoded_id = decoded_frames.fetch_add(1, Ordering::Relaxed) + 1;
            let _ = events.send(SessionEvent::FrameMetrics {
                bytes: bytes.len(),
                queue_ms: 0,
                decode_ms,
                dropped: 0,
            });
            let sid = format!("evrt-{decoded_id}");
            let _ = events.send(SessionEvent::Frame {
                sid,
                codec: codec.clone(),
                width: w,
                height: h,
                rgba,
            });
        }
    }

    evrt_log(&events, "EVRT decode loop exited".into());
}

// ─── декодирование H264 ────────────────────────────────────────────────────────

fn decode_h264_frame(
    bytes: &[u8],
    _width: u32,
    _height: u32,
    is_key: bool,
    vt: &mut Option<crate::videotoolbox::VideoToolboxH264Decoder>,
    #[cfg(feature = "live-h264")] sw: &mut Option<openh264::decoder::Decoder>,
    #[cfg(not(feature = "live-h264"))] _sw: &mut Option<()>,
    vt_failures: &mut u32,
) -> Option<(Vec<u8>, usize, usize)> {
    // Android: MediaCodec renders H264 directly to Surface (TextureView). No RGBA.
    #[cfg(all(target_os = "android", feature = "android-client"))]
    {
        crate::android_video::decode_frame_to_surface("H264", bytes, is_key, _width, _height);
        ANDROID_EVRT_FRAME_SEEN.store(true, Ordering::Relaxed);
        return None;
    }

    #[cfg(not(all(target_os = "android", feature = "android-client")))]
    let _ = is_key;

    const VT_FAIL_LIMIT: u32 = 5;

    // macOS VideoToolbox — API: decode_packets(iter) → (w, h, rgba)
    if let Some(ref mut dec) = vt {
        if *vt_failures < VT_FAIL_LIMIT {
            match dec.decode_packets(std::iter::once(bytes.to_vec())) {
                Ok(Some((w, h, rgba))) => {
                    *vt_failures = 0;
                    return Some((rgba, w, h));
                }
                Ok(None) => {
                    *vt_failures += 1;
                }
                Err(_) => {
                    *vt_failures += 1;
                }
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
    bytes: &[u8],
    width: u32,
    height: u32,
    is_key: bool,
    vt: &mut Option<crate::videotoolbox::VideoToolboxH264Decoder>,
    vt_failures: &mut u32,
    mf_dec: &mut Option<crate::mf_video::MfVideoDecoder>,
    mf_avail: bool,
) -> Option<(Vec<u8>, usize, usize)> {
    use crate::mf_video::MfVideoCodec;

    // Android: MediaCodec renders H265 directly to Surface (TextureView). No RGBA.
    #[cfg(all(target_os = "android", feature = "android-client"))]
    {
        crate::android_video::decode_frame_to_surface("H265", bytes, is_key, width, height);
        ANDROID_EVRT_FRAME_SEEN.store(true, Ordering::Relaxed);
        return None;
    }

    #[cfg(not(all(target_os = "android", feature = "android-client")))]
    let _ = is_key;

    const VT_FAIL_LIMIT: u32 = 5;

    // macOS VideoToolbox hardware HEVC — preferred when present.
    if let Some(ref mut dec) = vt {
        if *vt_failures < VT_FAIL_LIMIT {
            match dec.decode_packets(std::iter::once(bytes.to_vec())) {
                Ok(Some((w, h, rgba))) => {
                    *vt_failures = 0;
                    return Some((rgba, w, h));
                }
                Ok(None) | Err(_) => {
                    *vt_failures += 1;
                }
            }
        }
    }

    // Windows Media Foundation HEVC fallback.
    if !mf_avail {
        return None;
    }

    let dec = mf_dec.get_or_insert_with(|| {
        crate::mf_video::MfVideoDecoder::new(MfVideoCodec::H265, width, height)
            .expect("H265 MF decoder init")
    });

    match dec.decode_packets(std::iter::once(bytes.to_vec())) {
        Ok(Some((w, h, rgba))) => Some((rgba, w, h)),
        _ => None,
    }
}

// ─── декодирование VP9 ────────────────────────────────────────────────────────

fn decode_vp9_frame(
    bytes: &[u8],
    vp9_mf: &mut Option<crate::vp9_mf::Vp9MfDecoder>,
    events: &Sender<SessionEvent>,
) -> Option<(Vec<u8>, usize, usize)> {
    let dec = vp9_mf.as_mut()?;
    match dec.decode(bytes) {
        Ok(Some((w, h, rgba))) => Some((rgba, w, h)),
        Ok(None) => None, // декодер буферизует
        Err(e) => {
            evrt_log(events, format!("VP9 decode error: {e}"));
            None
        }
    }
}

// ─── декодирование AV1 ────────────────────────────────────────────────────────

fn decode_av1_frame(
    bytes: &[u8],
    width: u32,
    height: u32,
    is_key: bool,
    av1_mf: &mut Option<crate::mf_video::MfVideoDecoder>,
    events: &Sender<SessionEvent>,
) -> Option<(Vec<u8>, usize, usize)> {
    use crate::mf_video::MfVideoCodec;

    // Android: MediaCodec renders AV1 directly to Surface (TextureView). No RGBA.
    #[cfg(all(target_os = "android", feature = "android-client"))]
    {
        crate::android_video::decode_frame_to_surface("AV1", bytes, is_key, width, height);
        ANDROID_EVRT_FRAME_SEEN.store(true, Ordering::Relaxed);
        return None;
    }

    #[cfg(not(all(target_os = "android", feature = "android-client")))]
    let _ = is_key;

    if !crate::mf_video::mf_video_decode_status().av1 {
        evrt_log(
            events,
            "AV1: MF decoder not available on this system".into(),
        );
        return None;
    }

    let dec = av1_mf.get_or_insert_with(|| {
        crate::mf_video::MfVideoDecoder::new(MfVideoCodec::Av1, width, height)
            .expect("AV1 MF decoder init")
    });

    match dec.decode_packets(std::iter::once(bytes.to_vec())) {
        Ok(Some((w, h, rgba))) => Some((rgba, w, h)),
        _ => None,
    }
}

// ─── pressure ─────────────────────────────────────────────────────────────────

fn compute_pressure(
    arrival_delta_ms: i32,
    decode_delta_ms: i32,
    backlog: u32,
    new_drops: u64,
    cinema: bool,
    // На Android Surface MediaCodec имеет startup latency ~2-3 секунды,
    // поэтому короткий backlog (2-3 кадра) не означает реальную перегрузку.
    // Используем более мягкие пороги чтобы не триггерить relief на старте.
    android_surface: bool,
) -> Pressure {
    let (high_ms, crit_ms, backlog_crit, backlog_high) = if cinema {
        (30, 44, 3, 2)
    } else if android_surface {
        (22, 34, 6, 3) // Android: critical при 6+ кадрах в очереди, high при 3+
    } else {
        (22, 34, 2, 1)
    };

    // `arrival_delta_ms` is an inter-packet/frame gap. It grows on static
    // desktop content because the host legitimately sends fewer frames, so it
    // must not create receiver pressure by itself.
    let arrival_strained = arrival_delta_ms >= high_ms && (backlog > 0 || new_drops > 0);

    let crit = decode_delta_ms >= crit_ms || backlog >= backlog_crit || new_drops >= 3;

    let high = crit
        || arrival_strained
        || decode_delta_ms >= high_ms
        || backlog >= backlog_high
        || new_drops >= 1;

    if crit {
        Pressure::Critical
    } else if high {
        Pressure::High
    } else {
        Pressure::Normal
    }
}

// ─── вспомогательные ──────────────────────────────────────────────────────────

fn is_timeout(e: &std::io::Error) -> bool {
    e.kind() == std::io::ErrorKind::WouldBlock || e.kind() == std::io::ErrorKind::TimedOut
}

fn evrt_log(events: &Sender<SessionEvent>, msg: String) {
    eprintln!("[evrt-client] {msg}");
    let _ = events.send(SessionEvent::Info(msg));
}

// ─── интеграционный хелпер: пробовать EVRT перед TCP relay ───────────────────

/// Попробовать установить прямое EVRT-соединение.
/// Если хост не отвечает за `CONNECT_TIMEOUT` — вернуть `None` (нужен relay).
pub fn try_evrt_before_relay(
    local_udp: &Arc<UdpSocket>,
    host_addr: SocketAddr,
    session_token: Option<String>,
    events: &Sender<SessionEvent>,
    stop: Arc<AtomicBool>,
    ultra_low_lat: bool,
    audio_enabled: Arc<AtomicBool>,
) -> bool {
    let params = EvrtClientParams {
        socket: local_udp.clone(),
        host_addr,
        events: events.clone(),
        stop,
        ultra_low_latency: ultra_low_lat,
        session_token,
        audio_enabled,
        connect_timeout: CONNECT_TIMEOUT,
    };

    match run_evrt_client(params) {
        EvrtConnectResult::Ok => true,
        EvrtConnectResult::NoResponse => {
            evrt_log(
                events,
                "EVRT: no response — falling back to TCP relay".into(),
            );
            let _ = events.send(SessionEvent::EvrtStatus {
                active: false,
                host_addr: host_addr.ip().to_string(),
                port: host_addr.port(),
            });
            false
        }
        EvrtConnectResult::Error(e) => {
            evrt_log(
                events,
                format!("EVRT error ({e}) — falling back to TCP relay"),
            );
            let _ = events.send(SessionEvent::EvrtStatus {
                active: false,
                host_addr: host_addr.ip().to_string(),
                port: host_addr.port(),
            });
            false
        }
    }
}

/// mini-ICE: пробуем список кандидатов хоста (LAN + VPN + public/STUN) по очереди.
/// Первый ответивший — используем. Остальные пропускаем.
/// Каждый кандидат пробуется со своим свежим UDP-сокетом.
pub fn try_evrt_candidates(
    candidates: Vec<SocketAddr>,
    session_token: Option<String>,
    events: &Sender<SessionEvent>,
    ultra_low_lat: bool,
    stop: Arc<AtomicBool>,
    audio_enabled: Arc<AtomicBool>,
) -> bool {
    const CANDIDATE_CONNECT_TIMEOUT: Duration = Duration::from_millis(1100);

    for (i, addr) in candidates.iter().enumerate() {
        if stop.load(Ordering::Relaxed) {
            break;
        }
        evrt_log(
            events,
            format!(
                "EVRT кандидат {}/{}: пробуем {addr} (timeout={}ms)",
                i + 1,
                candidates.len(),
                CANDIDATE_CONNECT_TIMEOUT.as_millis()
            ),
        );
        let Ok(udp) = UdpSocket::bind("0.0.0.0:0") else {
            continue;
        };
        let udp = Arc::new(udp);

        // Быстрая проба: короткий таймаут на каждый кандидат, чтобы не висеть.
        // run_evrt_client сам делает CONNECT_TIMEOUT(4с); для перебора это ок,
        // т.к. правильный кандидат (VPN/LAN) ответит почти мгновенно.
        match run_evrt_client(EvrtClientParams {
            socket: udp,
            host_addr: *addr,
            events: events.clone(),
            stop: stop.clone(),
            ultra_low_latency: ultra_low_lat,
            session_token: session_token.clone(),
            audio_enabled: Arc::clone(&audio_enabled),
            connect_timeout: CANDIDATE_CONNECT_TIMEOUT,
        }) {
            EvrtConnectResult::Ok => {
                // Сессия завершилась нормально (была установлена)
                return true;
            }
            EvrtConnectResult::NoResponse => {
                evrt_log(events, format!("EVRT кандидат {addr}: нет ответа, дальше"));
                let _ = events.send(SessionEvent::EvrtStatus {
                    active: false,
                    host_addr: addr.ip().to_string(),
                    port: addr.port(),
                });
            }
            EvrtConnectResult::Error(e) => {
                evrt_log(
                    events,
                    format!("EVRT кандидат {addr}: ошибка ({e}), дальше"),
                );
                let _ = events.send(SessionEvent::EvrtStatus {
                    active: false,
                    host_addr: addr.ip().to_string(),
                    port: addr.port(),
                });
            }
        }
    }
    evrt_log(
        events,
        "EVRT: ни один кандидат не ответил — TCP relay".into(),
    );
    false
}

// ─── тесты ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pressure_normal_on_clean_stream() {
        let p = compute_pressure(5, 3, 0, 0, false, false);
        assert_eq!(p, Pressure::Normal);
    }

    #[test]
    fn pressure_high_on_backlog() {
        let p = compute_pressure(10, 10, 1, 0, false, false);
        assert_eq!(p, Pressure::High);
    }

    #[test]
    fn pressure_critical_on_drops() {
        let p = compute_pressure(40, 40, 3, 5, false, false);
        assert_eq!(p, Pressure::Critical);
    }

    #[test]
    fn pressure_normal_on_sparse_clean_stream() {
        let p = compute_pressure(125, 10, 0, 0, false, false);
        assert_eq!(p, Pressure::Normal);
    }

    #[test]
    fn cinema_mode_higher_thresholds() {
        // В game-режиме: decode=25 >= high_ms=22 → High
        let p_game = compute_pressure(25, 25, 0, 0, false, false);
        assert_eq!(p_game, Pressure::High);

        // В cinema-режиме: decode=25 < high_ms=30, backlog=0 → Normal
        let p_cinema = compute_pressure(25, 25, 0, 0, true, false);
        assert_eq!(p_cinema, Pressure::Normal);

        // В cinema-режиме: decode=35 >= high_ms=30 → High (но < crit_ms=44)
        let p_cinema_high = compute_pressure(35, 35, 0, 0, true, false);
        assert_eq!(p_cinema_high, Pressure::High);
    }
}
