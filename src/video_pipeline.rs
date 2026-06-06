// =============================================================================
// EVRT Protocol — разработан Артуром Валиевым (Artur Valiev)
// Rust-порт для EvertyDesk Lite
// =============================================================================

//! Единый видео-пайплайн.
//!
//! ```text
//! ┌─────────────┐    ┌─────────────┐
//! │ CaptureThread│───►│EncodeThread │
//! └─────────────┘    └──────┬──────┘
//!                           │ EncodedFrame
//!                    ┌──────▼──────┐
//!                    │  Dispatcher │
//!                    └──┬──────┬───┘
//!              ┌────────┘      └────────┐
//!        ┌─────▼──────┐        ┌────────▼───────┐
//!        │  TcpSender │        │  EvrtUdpSender  │
//!        │ (fallback) │        │  (primary)      │
//!        └────────────┘        └─────────────────┘
//! ```
//!
//! - **Один** MF-энкодер — нет race conditions
//! - **Один** capture тред — нет двойной нагрузки
//! - TcpSender отправляет IDR-only когда EVRT активен
//! - EvrtSender получает все кадры, упаковывает в EVRT UDP

use std::{
    net::{SocketAddr, UdpSocket},
    sync::{
        atomic::{AtomicBool, AtomicU32, Ordering},
        mpsc::{self, Receiver, Sender, SyncSender},
        Arc, Mutex,
    },
    thread,
    time::{Duration, Instant},
};

use crate::{
    host::{ClientVideoSupport, HostEvent},
    settings::AppConfig,
};

// ─── Encoded frame ────────────────────────────────────────────────────────────

/// Закодированный кадр — единица данных через весь пайплайн.
#[derive(Clone)]
pub struct EncodedFrame {
    pub bytes: Arc<Vec<u8>>,
    pub is_idr: bool,
    pub frame_id: u32,
    pub pts_us: u64,
    pub display: i32,
    /// SPS/PPS — только для IDR кадров.
    pub sps_pps: Option<Arc<Vec<u8>>>,
    pub width: u32,
    pub height: u32,
    pub codec: &'static str,
    pub roi: crate::evrt::RoiRect,
}

/// Элемент TCP-канала: видеокадр или control-сообщение (shell output).
enum TcpItem {
    Video(EncodedFrame),
    Peer(crate::rustdesk_proto::PeerMessage),
}

// ─── Pipeline commands ────────────────────────────────────────────────────────

pub enum PipelineCmd {
    Stop,
    SetFps(u32),
    SetQuality(u32),
    SetDisplay(i32),
    RequestIdr,
    EvrtPeerConnected(SocketAddr),
    EvrtSessionEnded,
}

// ─── Pipeline config ──────────────────────────────────────────────────────────

pub struct PipelineConfig {
    pub app_config: AppConfig,
    pub peer_id: String,
    pub events: Sender<HostEvent>,
    pub client_video: ClientVideoSupport,
    pub relay_stream: std::net::TcpStream,
    /// Шифрование исходящего TCP потока (видео).
    /// RecvCipher остаётся в relay_session_inner для расшифровки управляющих сообщений.
    pub send_cipher: Option<crate::crypto::SendCipher>,
    pub evrt_socket: Option<Arc<UdpSocket>>,
    pub cmd_rx: Receiver<PipelineCmd>,
    pub peer_msg_rx: Receiver<crate::rustdesk_proto::PeerMessage>,
}

// ─── run() ───────────────────────────────────────────────────────────────────

pub fn run(cfg: PipelineConfig) {
    let PipelineConfig {
        app_config,
        peer_id,
        events,
        client_video,
        relay_stream,
        send_cipher,
        evrt_socket,
        cmd_rx,
        peer_msg_rx,
    } = cfg;

    let stop = Arc::new(AtomicBool::new(false));
    let target_fps = Arc::new(AtomicU32::new(app_config.display.target_fps.clamp(5, 60)));
    let quality_ms = Arc::new(AtomicU32::new(1_000));
    let bitrate_scale_milli = Arc::new(AtomicU32::new(1_000));
    let active_display = Arc::new(AtomicU32::new(0));

    // IDR request channel: cmd_rx → encoder
    let (idr_tx, idr_rx) = mpsc::channel::<()>();

    // ── TCP канал несёт И видео, И control (shell output) ──────────────────────
    // TcpItem::Video — видеокадры (с приоритетом latency, буфер 2)
    // TcpItem::Peer  — shell output, control сообщения (всегда доставляются)
    // Буфер 4: при блокирующем send даёт backpressure без большой задержки
    // (≤4 кадра в полёте ≈ 130мс на 30fps). EVRT-канал меньше — UDP latency-critical.
    let (tcp_tx, tcp_rx) = mpsc::sync_channel::<TcpItem>(4);
    let (evrt_tx, evrt_rx) = mpsc::sync_channel::<EncodedFrame>(2);
    let mut worker_handles = Vec::new();

    // ── Флаг: EVRT активен? Устанавливается когда клиент прислал punch ────────
    let evrt_active: Arc<Mutex<Option<SocketAddr>>> = Arc::new(Mutex::new(None));

    // ── Forwarder: peer_msg_rx (shell output) → TCP канал ────────────────────
    {
        let tcp_fwd = tcp_tx.clone();
        let stop_fwd = stop.clone();
        let handle = thread::Builder::new()
            .name("pipeline-peermsg".into())
            .spawn(move || {
                while !stop_fwd.load(Ordering::Relaxed) {
                    match peer_msg_rx.recv_timeout(Duration::from_millis(100)) {
                        Ok(msg) => {
                            // Shell output всегда идёт по TCP relay (не video path)
                            match tcp_fwd.try_send(TcpItem::Peer(msg)) {
                                Ok(()) | Err(mpsc::TrySendError::Full(_)) => {}
                                Err(mpsc::TrySendError::Disconnected(_)) => break,
                            }
                        }
                        Err(mpsc::RecvTimeoutError::Timeout) => {}
                        Err(_) => break,
                    }
                }
            })
            .expect("spawn peermsg-forwarder");
        worker_handles.push(handle);
    }

    // ── Encoder + Capture thread ─────────────────────────────────────────────
    {
        let stop_e = stop.clone();
        let fps_e = target_fps.clone();
        let qual_e = quality_ms.clone();
        let bitrate_scale_e = bitrate_scale_milli.clone();
        let display_e = active_display.clone();
        let cfg_e = app_config.clone();
        let client_video_e = client_video;
        let ev_e = events.clone();
        let tcp_e = tcp_tx;
        let evrt_e = evrt_tx;
        let act_e = evrt_active.clone();

        let handle = thread::Builder::new()
            .name("pipeline-encoder".into())
            .spawn(move || {
                encode_loop(
                    stop_e,
                    fps_e,
                    qual_e,
                    bitrate_scale_e,
                    display_e,
                    cfg_e,
                    client_video_e,
                    ev_e,
                    tcp_e,
                    evrt_e,
                    act_e,
                    idr_rx,
                );
            })
            .expect("spawn encoder");
        worker_handles.push(handle);
    }

    // ── TCP Sender thread ─────────────────────────────────────────────────────
    {
        let stop_t = stop.clone();
        let ev_t = events.clone();
        let mut stream = relay_stream;
        let mut cipher = send_cipher;
        let act_t = evrt_active.clone();

        let handle = thread::Builder::new()
            .name("pipeline-tcp".into())
            .spawn(move || {
                tcp_send_loop(stop_t, tcp_rx, &mut stream, &mut cipher, act_t, ev_t);
            })
            .expect("spawn tcp-sender");
        worker_handles.push(handle);
    }

    // ── EVRT UDP Sender thread ────────────────────────────────────────────────
    if let Some(udp_sock) = evrt_socket {
        let stop_u = stop.clone();
        let ev_u = events.clone();
        let cfg_u = app_config.clone();
        let fps_u = target_fps.clone();
        let qual_u = quality_ms.clone();
        let bitrate_scale_u = bitrate_scale_milli.clone();
        let pid_u = peer_id.clone();
        let act_u = evrt_active.clone();

        let handle = thread::Builder::new()
            .name("pipeline-evrt".into())
            .spawn(move || {
                evrt_send_loop(
                    stop_u,
                    udp_sock,
                    evrt_rx,
                    act_u,
                    ev_u,
                    cfg_u,
                    fps_u,
                    qual_u,
                    bitrate_scale_u,
                    pid_u,
                );
            })
            .expect("spawn evrt-sender");
        worker_handles.push(handle);
    } else {
        // Нет EVRT сокета — дропаем evrt_rx чтобы encoder не блокировался
        worker_handles.push(thread::spawn(move || while evrt_rx.recv().is_ok() {}));
    }

    // ── Command loop (этот тред) ───────────────────────────────────────────────
    while !stop.load(Ordering::Relaxed) {
        match cmd_rx.recv_timeout(Duration::from_millis(50)) {
            Ok(PipelineCmd::Stop) => {
                stop.store(true, Ordering::Relaxed);
                break;
            }
            Ok(PipelineCmd::SetFps(fps)) => {
                target_fps.store(fps.clamp(5, 60), Ordering::Relaxed);
            }
            Ok(PipelineCmd::SetQuality(q)) => {
                quality_ms.store(q, Ordering::Relaxed);
            }
            Ok(PipelineCmd::SetDisplay(display)) => {
                let display = display.max(0);
                active_display.store(display as u32, Ordering::Relaxed);
                let _ = idr_tx.send(());
                log(
                    &events,
                    format!("Pipeline: switched capture to display {}", display + 1),
                );
            }
            Ok(PipelineCmd::RequestIdr) => {
                let _ = idr_tx.send(());
            }
            Ok(PipelineCmd::EvrtPeerConnected(addr)) => {
                if let Ok(mut g) = evrt_active.lock() {
                    *g = Some(addr);
                }
                log(&events, format!("Pipeline: EVRT активен → {addr}"));
            }
            Ok(PipelineCmd::EvrtSessionEnded) => {
                if let Ok(mut g) = evrt_active.lock() {
                    *g = None;
                }
                log(&events, "Pipeline: EVRT завершён, TCP relay primary".into());
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                stop.store(true, Ordering::Relaxed);
                break;
            }
        }
    }

    stop.store(true, Ordering::Relaxed);
    let join_events = events.clone();
    let join_peer = peer_id.clone();
    thread::Builder::new()
        .name("pipeline-joiner".into())
        .spawn(move || {
            for handle in worker_handles {
                let _ = handle.join();
            }
            log(
                &join_events,
                format!("Pipeline workers joined for {join_peer}"),
            );
        })
        .ok();

    log(&events, format!("Pipeline для {peer_id} завершён"));
}

// ─── Encode + Capture loop ────────────────────────────────────────────────────

fn encode_loop(
    stop: Arc<AtomicBool>,
    target_fps: Arc<AtomicU32>,
    quality_ms: Arc<AtomicU32>,
    bitrate_scale_milli: Arc<AtomicU32>,
    active_display: Arc<AtomicU32>,
    config: AppConfig,
    client_video: ClientVideoSupport,
    events: Sender<HostEvent>,
    tcp_tx: SyncSender<TcpItem>,
    evrt_tx: SyncSender<EncodedFrame>,
    evrt_active: Arc<Mutex<Option<SocketAddr>>>,
    idr_rx: Receiver<()>,
) {
    use crate::host::{h264_target_bitrate_bps_pub, MultiEncoder};

    log(&events, "Encoder loop started".into());

    // ★ Единый каскад энкодеров: MF → VideoToolbox → NVENC → OpenH264 → PNG
    let mut encoder = MultiEncoder::new(config.display.encoder, config.display.codec, client_video);
    log(
        &events,
        format!("Encoder каскад: {}", encoder.backend_label()),
    );

    // FSR
    let mut fsr = config.display.fsr_quality.to_fsr_quality().map(|q| {
        crate::fsr::FsrAdapter::new(crate::fsr::FsrConfig {
            quality: q,
            sharpness: config.display.fsr_sharpness,
        })
    });

    // HNS PTS counter
    let mut sample_hns: u64 = 0;
    let hns_per_frame = |fps: u32| 10_000_000u64 / fps.max(1) as u64;

    // ★ Детектор изменений — пропускает статичные кадры (экономия трафика)
    let mut change_detector = crate::host::FrameChangeDetector::default();

    // ★ Телеметрия pipeline
    let mut tele = PipelineTelemetry::new(encoder.backend_label());
    let mut last_tele_at = Instant::now();
    let mut backend_logged = false;
    const TELE_INTERVAL: Duration = Duration::from_secs(10);

    // Capture double buffer
    type CapSlot = Arc<Mutex<Option<(i32, u32, u32, Vec<u8>)>>>;
    let cap_slot: CapSlot = Arc::new(Mutex::new(None));
    // Keep the handle so we can join the capture thread before NVENC is
    // destroyed.  Without this join, the DXGI duplication and NVENC D3D11
    // devices overlap during teardown → GPU TDR → system freeze.
    let cap_handle = {
        let cap_stop = stop.clone();
        let cap_fps = target_fps.clone();
        let cap_display = active_display.clone();
        let cap_slot2 = cap_slot.clone();
        thread::Builder::new()
            .name("pipeline-capture".into())
            .spawn(move || {
                let mut buf = Vec::new();
                loop {
                    if cap_stop.load(Ordering::Relaxed) {
                        break;
                    }
                    let fps = cap_fps.load(Ordering::Relaxed).clamp(5, 60);
                    let display = cap_display.load(Ordering::Relaxed) as i32;
                    if let Some((w, h)) = crate::capture::capture_display_into(display, &mut buf) {
                        if let Ok(mut slot) = cap_slot2.lock() {
                            match slot.as_mut() {
                                Some(s) if s.0 == display && s.1 == w && s.2 == h => {
                                    std::mem::swap(&mut s.3, &mut buf);
                                }
                                _ => *slot = Some((display, w, h, buf.clone())),
                            }
                        }
                    }
                    let us = 1_000_000u64 / fps.max(1) as u64;
                    thread::sleep(Duration::from_micros(us.saturating_sub(500)));
                }
            })
            .expect("spawn capture")
    };

    let mut frame_id: u32 = 0;
    let mut last_idr: Instant = Instant::now();
    let mut next_frame_due: Instant = Instant::now();
    const IDR_MIN: Duration = Duration::from_secs(2);
    const SPIN: Duration = Duration::from_micros(1_500);

    while !stop.load(Ordering::Relaxed) {
        let fps = target_fps.load(Ordering::Relaxed).clamp(5, 60);
        let frame_interval = Duration::from_nanos(1_000_000_000 / fps as u64);

        // Точный тайминг (nextFrameDueTicks из EvertyGame)
        let now = Instant::now();
        if now < next_frame_due {
            let wait = next_frame_due - now;
            if wait > SPIN {
                thread::sleep(wait - SPIN);
            } else {
                std::hint::spin_loop();
            }
            continue;
        }
        next_frame_due += frame_interval;
        if next_frame_due < Instant::now() {
            next_frame_due = Instant::now() + frame_interval;
        }

        // IDR по таймеру/запросу
        let periodic_key = idr_rx.try_recv().is_ok() || last_idr.elapsed() > IDR_MIN;

        // Захват
        let cap_started = Instant::now();
        let Some((display, cap_w, cap_h, bgra_raw)) =
            cap_slot.lock().ok().and_then(|mut s| s.take())
        else {
            thread::sleep(Duration::from_millis(1));
            continue;
        };
        tele.mark_capture(cap_started.elapsed());

        // FSR: апскейл происходит in-place в буфере адаптера.
        // Передаём срез напрямую в кодировщик — без лишнего .to_owned()
        // (кодирование синхронно сразу после, буфер FSR жив весь кадр).
        let (enc_w, enc_h) = (cap_w, cap_h);
        let bgra: &[u8] = match fsr {
            Some(ref mut a) => a.process_bgra(&bgra_raw, cap_w, cap_h, enc_w, enc_h),
            None => &bgra_raw,
        };

        // ── Детекция изменений: пропускаем статичные кадры ────────────────────
        let change_started = Instant::now();
        let decision = change_detector.decide(enc_w, enc_h, bgra, periodic_key);
        tele.mark_change(change_started.elapsed());

        if !decision.send {
            // Кадр не изменился — не кодируем, не шлём. Экономия трафика и CPU.
            tele.mark_skipped();
            // Backoff при долгой статике — снижаем частоту опроса
            if let Some(delay) = change_detector.static_backoff_delay(fps) {
                thread::sleep(delay);
            }
            maybe_emit_telemetry(
                &events,
                &mut tele,
                &mut change_detector,
                &mut last_tele_at,
                TELE_INTERVAL,
                fps,
            );
            continue;
        }

        let want_idr = decision.force_key || periodic_key;
        let quality = quality_ms.load(Ordering::Relaxed);
        let base_bps = h264_target_bitrate_bps_pub(enc_w, enc_h, fps, quality);
        let relief_milli = bitrate_scale_milli
            .load(Ordering::Relaxed)
            .clamp(MIN_BITRATE_SCALE_MILLI, 1_000);
        let mut eff_bps =
            adapt_bitrate(base_bps, decision.roi, enc_w, enc_h, want_idr, relief_milli);

        // ★ Cap битрейта под транспорт.
        //   EVRT активен (прямой UDP по LAN) → полный битрейт, сеть быстрая.
        //   EVRT НЕ активен (TCP relay) → relay тянет ~5-8 Мбит/с, выше — захлёб.
        //   RustDesk целится в 3-6 Мбит/с на relay; делаем так же.
        let evrt_on = evrt_active.lock().map(|g| g.is_some()).unwrap_or(false);
        if !evrt_on {
            const RELAY_MAX_BPS: u32 = 5_000_000; // безопасно для hbbr relay
            eff_bps = eff_bps.min(RELAY_MAX_BPS);
        }

        // ── Кодирование через единый каскад ───────────────────────────────────
        let encode_started = Instant::now();
        let Some(out) = encoder.encode(enc_w, enc_h, fps, eff_bps, bgra, want_idr) else {
            continue;
        };
        let encode_dur = encode_started.elapsed();
        tele.mark_encode(encode_dur);

        // ★ Один раз логируем РЕАЛЬНЫЙ бэкенд (MediaFoundation/OpenH264-SW/PNG).
        //   Критично для диагностики: показывает, аппаратный энкодер или софт.
        if !backend_logged {
            backend_logged = true;
            log(
                &events,
                format!(
                    "★ Реальный энкодер: {} ({}×{}@{}, первый кадр {}мс)",
                    encoder.active_backend(),
                    enc_w,
                    enc_h,
                    fps,
                    encode_dur.as_millis(),
                ),
            );
            // Если MF упал и мы на софте — печатаем ПОЧЕМУ MF не сработал.
            if let Some(err) = encoder.take_mf_error() {
                log(&events, format!("★ MF отключён, причина: {err}"));
            }
        }

        frame_id = frame_id.wrapping_add(1);
        let pts_us = sample_hns / 10;
        sample_hns = sample_hns.wrapping_add(hns_per_frame(fps));
        let mut roi = decision.roi;
        roi.frame_id = frame_id;

        let is_idr = out.key;
        if is_idr {
            last_idr = Instant::now();
        }

        // Отметить кадр как отправленный в детекторе
        tele.mark_sent(out.bytes.len(), is_idr);
        tele.mark_bitrate(
            decision.roi.dirty_area_milli(enc_w, enc_h),
            eff_bps,
            relief_milli,
        );
        change_detector.mark_sent(enc_w, enc_h, bgra);

        let frame = EncodedFrame {
            bytes: Arc::new(out.bytes),
            is_idr,
            frame_id,
            pts_us,
            display,
            sps_pps: out.sps_pps.map(Arc::new),
            width: enc_w,
            height: enc_h,
            codec: out.codec,
            roi,
        };

        maybe_emit_telemetry(
            &events,
            &mut tele,
            &mut change_detector,
            &mut last_tele_at,
            TELE_INTERVAL,
            fps,
        );

        // ── Dispatch ──────────────────────────────────────────────────────────
        // EVRT активен: все кадры → EVRT, только IDR → TCP (синхронизация)
        // EVRT не активен: все кадры → TCP
        // (evrt_on уже вычислен выше для cap битрейта)
        if evrt_on {
            // EVRT primary: все кадры → EVRT, IDR → TCP для синхронизации
            match evrt_tx.try_send(frame.clone()) {
                Ok(()) => {}
                Err(mpsc::TrySendError::Full(_)) => {} // клиент не успевает
                Err(mpsc::TrySendError::Disconnected(_)) => break,
            }
            if is_idr {
                match tcp_tx.try_send(TcpItem::Video(frame)) {
                    Ok(()) | Err(mpsc::TrySendError::Full(_)) => {}
                    Err(mpsc::TrySendError::Disconnected(_)) => break,
                }
            }
        } else {
            // TCP relay is bounded and latency-sensitive. Drop video when the
            // sender is backed up so control messages and shutdown can still
            // make progress; the next captured frame will be fresher anyway.
            match tcp_tx.try_send(TcpItem::Video(frame)) {
                Ok(()) | Err(mpsc::TrySendError::Full(_)) => {}
                Err(mpsc::TrySendError::Disconnected(_)) => break,
            }
        }
    }

    stop.store(true, Ordering::Relaxed);

    // Do NOT call everty_nvenc_destroy here.
    // Destroying a D3D11 device (used by NVENC) while WGPU renders via D3D12
    // causes a deadlock inside nvwgf2umx.dll — both codepaths fight for the
    // same NVIDIA internal critical section. The render thread freezes, the
    // Win32 message pump stops, WM_CLOSE is never processed.
    // Intentionally leaking the encoder avoids this entirely. The OS frees
    // all VRAM when the process exits (normal or via TerminateProcess watchdog).
    encoder.leak_gpu_resources();
    let _ = cap_handle; // keep alive until capture notices stop (~100 ms)

    log(&events, "Encoder loop stopped".into());
}

// ─── TCP Sender loop ──────────────────────────────────────────────────────────

fn tcp_send_loop(
    stop: Arc<AtomicBool>,
    item_rx: Receiver<TcpItem>,
    stream: &mut std::net::TcpStream,
    cipher: &mut Option<crate::crypto::SendCipher>,
    _evrt_active: Arc<Mutex<Option<SocketAddr>>>,
    events: Sender<HostEvent>,
) {
    // Write timeout чтобы не висеть при разрыве соединения
    stream.set_write_timeout(Some(Duration::from_secs(2))).ok();
    log(&events, "TCP sender started".into());

    while !stop.load(Ordering::Relaxed) {
        let item = match item_rx.recv_timeout(Duration::from_millis(50)) {
            Ok(i) => i,
            Err(mpsc::RecvTimeoutError::Timeout) => continue,
            Err(_) => break,
        };

        // Видео → конвертируем в VideoFrame; control (shell) → как есть
        let msg = match item {
            TcpItem::Video(frame) => make_tcp_video_frame(&frame),
            TcpItem::Peer(peer_msg) => peer_msg,
        };

        let mut payload = crate::transport::encode_peer_message_raw(&msg);
        if let Some(c) = cipher.as_mut() {
            payload = c.encrypt(&payload);
        }
        if crate::transport::send_framed_raw(stream, &payload).is_err() {
            break;
        }
    }

    stop.store(true, Ordering::Relaxed);
    log(&events, "TCP sender stopped".into());
}

fn make_tcp_video_frame(frame: &EncodedFrame) -> crate::rustdesk_proto::PeerMessage {
    use crate::rustdesk_proto::{
        peer_message, video_frame, EncodedVideoFrame, EncodedVideoFrames, PeerMessage, VideoFrame,
    };
    let encoded = EncodedVideoFrame {
        data: (*frame.bytes).clone(),
        key: frame.is_idr,
        pts: frame.pts_us as i64,
    };
    let frames = EncodedVideoFrames {
        frames: vec![encoded],
    };
    let union = match frame.codec {
        "H265" => video_frame::Union::H265s(frames),
        _ => video_frame::Union::H264s(frames),
    };
    PeerMessage {
        union: Some(peer_message::Union::VideoFrame(VideoFrame {
            union: Some(union),
            display: frame.display,
        })),
    }
}

// ─── EVRT UDP Sender loop ─────────────────────────────────────────────────────

fn evrt_send_loop(
    stop: Arc<AtomicBool>,
    socket: Arc<UdpSocket>,
    frame_rx: Receiver<EncodedFrame>,
    evrt_active: Arc<Mutex<Option<SocketAddr>>>,
    events: Sender<HostEvent>,
    config: AppConfig,
    target_fps: Arc<AtomicU32>,
    quality_ms: Arc<AtomicU32>,
    bitrate_scale_milli: Arc<AtomicU32>,
    peer_id: String,
) {
    log(&events, "EVRT UDP sender: ожидание punch…".into());

    // Ждём punch от клиента
    let deadline = Instant::now() + Duration::from_secs(8);
    let mut buf = vec![0u8; crate::evrt::MAX_PACKET_SIZE + 64];
    socket
        .set_read_timeout(Some(Duration::from_millis(300)))
        .ok();

    let peer_addr = loop {
        if stop.load(Ordering::Relaxed) || Instant::now() > deadline {
            log(
                &events,
                "EVRT: punch timeout — UDP сессия не запущена".into(),
            );
            log(&events, "EVRT UDP sender stopped".into());
            return;
        }
        if let Ok((_, src)) = socket.recv_from(&mut buf) {
            log(&events, format!("EVRT: punch от {src}"));
            if let Ok(mut g) = evrt_active.lock() {
                *g = Some(src);
            }
            break src;
        }
    };

    // Запускаем EVRT сессию, передаём frame_rx из pipeline
    let params = crate::evrt_session::EvrtSessionParams {
        peer_addr,
        socket: socket.clone(),
        config: config.clone(),
        peer_id: peer_id.clone(),
        events: events.clone(),
        stop: stop.clone(),
        frame_rx, // ← готовые кадры из encoder, не своя запись
        target_fps,
        quality_milli: quality_ms,
        bitrate_scale_milli,
    };

    if let Err(e) = crate::evrt_session::run_evrt_session(params) {
        log(&events, format!("EVRT session ended: {e}"));
    }

    // EVRT завершён
    if let Ok(mut g) = evrt_active.lock() {
        *g = None;
    }
    log(&events, "EVRT UDP sender stopped".into());
}

// ─── Телеметрия pipeline ──────────────────────────────────────────────────────

/// Метрики кодирования за интервал. Эмитится в UI каждые 10 сек.
struct PipelineTelemetry {
    backend: String,
    sent_frames: u64,
    skipped_frames: u64,
    sent_bytes: u64,
    keyframes: u64,
    roi_milli_total: u64,
    roi_milli_max: u32,
    relief_milli_total: u64,
    relief_milli_min: u32,
    bitrate_bps_total: u64,
    bitrate_bps_min: u32,
    bitrate_bps_max: u32,
    capture_us_total: u64,
    change_us_total: u64,
    encode_us_total: u64,
    samples: u64,
    capture_us_max: u64,
    encode_us_max: u64,
}

impl PipelineTelemetry {
    fn new(backend: String) -> Self {
        Self {
            backend,
            sent_frames: 0,
            skipped_frames: 0,
            sent_bytes: 0,
            keyframes: 0,
            roi_milli_total: 0,
            roi_milli_max: 0,
            relief_milli_total: 0,
            relief_milli_min: 0,
            bitrate_bps_total: 0,
            bitrate_bps_min: 0,
            bitrate_bps_max: 0,
            capture_us_total: 0,
            change_us_total: 0,
            encode_us_total: 0,
            samples: 0,
            capture_us_max: 0,
            encode_us_max: 0,
        }
    }
    fn mark_capture(&mut self, d: Duration) {
        let us = d.as_micros() as u64;
        self.capture_us_total += us;
        self.capture_us_max = self.capture_us_max.max(us);
        self.samples += 1;
    }
    fn mark_change(&mut self, d: Duration) {
        self.change_us_total += d.as_micros() as u64;
    }
    fn mark_encode(&mut self, d: Duration) {
        let us = d.as_micros() as u64;
        self.encode_us_total += us;
        self.encode_us_max = self.encode_us_max.max(us);
    }
    fn mark_sent(&mut self, bytes: usize, is_idr: bool) {
        self.sent_frames += 1;
        self.sent_bytes += bytes as u64;
        if is_idr {
            self.keyframes += 1;
        }
    }
    fn mark_bitrate(&mut self, roi_milli: u32, bitrate_bps: u32, relief_milli: u32) {
        self.roi_milli_total += u64::from(roi_milli.min(1_000));
        self.roi_milli_max = self.roi_milli_max.max(roi_milli.min(1_000));
        let relief_milli = relief_milli.clamp(MIN_BITRATE_SCALE_MILLI, 1_000);
        self.relief_milli_total += u64::from(relief_milli);
        self.relief_milli_min = if self.relief_milli_min == 0 {
            relief_milli
        } else {
            self.relief_milli_min.min(relief_milli)
        };
        self.bitrate_bps_total += u64::from(bitrate_bps);
        self.bitrate_bps_min = if self.bitrate_bps_min == 0 {
            bitrate_bps
        } else {
            self.bitrate_bps_min.min(bitrate_bps)
        };
        self.bitrate_bps_max = self.bitrate_bps_max.max(bitrate_bps);
    }
    fn mark_skipped(&mut self) {
        self.skipped_frames += 1;
    }
    fn reset(&mut self) {
        self.sent_frames = 0;
        self.skipped_frames = 0;
        self.sent_bytes = 0;
        self.keyframes = 0;
        self.roi_milli_total = 0;
        self.roi_milli_max = 0;
        self.relief_milli_total = 0;
        self.relief_milli_min = 0;
        self.bitrate_bps_total = 0;
        self.bitrate_bps_min = 0;
        self.bitrate_bps_max = 0;
        self.capture_us_total = 0;
        self.change_us_total = 0;
        self.encode_us_total = 0;
        self.samples = 0;
        self.capture_us_max = 0;
        self.encode_us_max = 0;
    }
}

fn maybe_emit_telemetry(
    events: &Sender<HostEvent>,
    tele: &mut PipelineTelemetry,
    _change: &mut crate::host::FrameChangeDetector,
    last_at: &mut Instant,
    interval: Duration,
    fps: u32,
) {
    if last_at.elapsed() < interval {
        return;
    }

    let s = tele.samples.max(1);
    let sent = tele.sent_frames.max(1);
    let avg_cap = tele.capture_us_total / s / 1000; // ms
    let avg_change = tele.change_us_total / s / 1000;
    let avg_enc = tele.encode_us_total / sent / 1000;
    let avg_bytes = tele.sent_bytes / sent;
    let kbps = (tele.sent_bytes * 8) / 1000 / interval.as_secs().max(1);
    let bitrate_avg_kbps = tele.bitrate_bps_total / sent / 1000;
    let bitrate_min_kbps = u64::from(tele.bitrate_bps_min) / 1000;
    let bitrate_max_kbps = u64::from(tele.bitrate_bps_max) / 1000;
    let roi_avg_pct = (tele.roi_milli_total / sent).min(1_000) / 10;
    let roi_max_pct = u64::from(tele.roi_milli_max.min(1_000)) / 10;
    let relief_avg_pct = (tele.relief_milli_total / sent).min(1_000) / 10;
    let relief_min_pct = u64::from(tele.relief_milli_min.min(1_000)) / 10;

    let summary = format!(
        "backend={} fps={} sent={} skipped_static={} keyframes={} \
         avg_packet={}B bitrate={}kbps bitrate_min={}kbps bitrate_max={}kbps \
         roi_avg={}pct roi_max={}pct relief={}pct relief_min={}pct \
         actual={}kbps capture_avg={}ms capture_max={}ms \
         change_avg={}ms encode_avg={}ms encode_max={}ms",
        tele.backend,
        fps,
        tele.sent_frames,
        tele.skipped_frames,
        tele.keyframes,
        avg_bytes,
        bitrate_avg_kbps,
        bitrate_min_kbps,
        bitrate_max_kbps,
        roi_avg_pct,
        roi_max_pct,
        relief_avg_pct,
        relief_min_pct,
        kbps,
        avg_cap,
        tele.capture_us_max / 1000,
        avg_change,
        avg_enc,
        tele.encode_us_max / 1000,
    );

    eprintln!("[pipeline] telemetry: {summary}");
    let _ = events.send(HostEvent::VideoTelemetry {
        summary,
        fallback_reason: None,
    });

    tele.reset();
    *last_at = Instant::now();
}

// ─── helpers ─────────────────────────────────────────────────────────────────

fn log(events: &Sender<HostEvent>, msg: String) {
    eprintln!("[pipeline] {msg}");
    let _ = events.send(HostEvent::Log(msg));
}

const ROI_BITRATE_MIN_BPS: u32 = 600_000;
const BITRATE_QUANTUM_BPS: u32 = 100_000;
const MIN_BITRATE_SCALE_MILLI: u32 = 200;

fn adapt_bitrate(
    base_bps: u32,
    roi: crate::evrt::RoiRect,
    width: u32,
    height: u32,
    force_full_roi_quality: bool,
    network_scale_milli: u32,
) -> u32 {
    if base_bps == 0 {
        return base_bps;
    }

    let roi_scale_milli = if force_full_roi_quality || roi.is_full_screen() {
        1_000
    } else {
        roi_bitrate_scale_milli(roi.dirty_area_milli(width, height))
    };
    let network_scale_milli = network_scale_milli.clamp(MIN_BITRATE_SCALE_MILLI, 1_000);
    let scale_milli = (u64::from(roi_scale_milli) * u64::from(network_scale_milli) / 1_000)
        .clamp(u64::from(MIN_BITRATE_SCALE_MILLI), 1_000) as u32;
    if scale_milli >= 1_000 {
        return base_bps;
    }

    let min_bps = ROI_BITRATE_MIN_BPS.min(base_bps);
    let scaled = (u64::from(base_bps) * u64::from(scale_milli) / 1_000)
        .clamp(u64::from(min_bps), u64::from(base_bps)) as u32;
    quantize_bitrate(scaled, min_bps, base_bps)
}

fn roi_bitrate_scale_milli(dirty_milli: u32) -> u32 {
    match dirty_milli.min(1_000) {
        0..=20 => 450,
        21..=80 => 550,
        81..=200 => 700,
        201..=450 => 850,
        _ => 1_000,
    }
}

fn quantize_bitrate(bps: u32, min_bps: u32, max_bps: u32) -> u32 {
    if BITRATE_QUANTUM_BPS == 0 {
        return bps.clamp(min_bps, max_bps);
    }
    let q = BITRATE_QUANTUM_BPS;
    let rounded = ((bps.saturating_add(q / 2)) / q).saturating_mul(q);
    rounded.clamp(min_bps, max_bps)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roi_adaptation_keeps_fullscreen_at_base_bitrate() {
        let base = 8_500_000;
        let roi = crate::evrt::RoiRect {
            frame_id: 1,
            x: 0,
            y: 0,
            w: 0,
            h: 0,
        };
        assert_eq!(adapt_bitrate(base, roi, 1920, 1080, false, 1_000), base);
    }

    #[test]
    fn roi_adaptation_keeps_idr_at_base_bitrate() {
        let base = 8_500_000;
        let roi = crate::evrt::RoiRect {
            frame_id: 1,
            x: 100,
            y: 100,
            w: 64,
            h: 64,
        };
        assert_eq!(adapt_bitrate(base, roi, 1920, 1080, true, 1_000), base);
    }

    #[test]
    fn roi_adaptation_reduces_small_dirty_region() {
        let base = 8_500_000;
        let roi = crate::evrt::RoiRect {
            frame_id: 1,
            x: 100,
            y: 100,
            w: 64,
            h: 64,
        };
        let adapted = adapt_bitrate(base, roi, 1920, 1080, false, 1_000);
        assert!(adapted < base);
        assert_eq!(adapted, 3_800_000);
    }

    #[test]
    fn roi_adaptation_never_goes_below_floor() {
        let base = 800_000;
        let roi = crate::evrt::RoiRect {
            frame_id: 1,
            x: 10,
            y: 10,
            w: 16,
            h: 16,
        };
        assert_eq!(adapt_bitrate(base, roi, 3840, 2160, false, 1_000), 600_000);
    }

    #[test]
    fn network_relief_reduces_idr_bitrate() {
        let base = 8_500_000;
        let roi = crate::evrt::RoiRect {
            frame_id: 1,
            x: 0,
            y: 0,
            w: 0,
            h: 0,
        };
        assert_eq!(adapt_bitrate(base, roi, 1920, 1080, true, 880), 7_500_000);
    }

    #[test]
    fn roi_and_network_relief_compose() {
        let base = 8_500_000;
        let roi = crate::evrt::RoiRect {
            frame_id: 1,
            x: 100,
            y: 100,
            w: 64,
            h: 64,
        };
        assert_eq!(adapt_bitrate(base, roi, 1920, 1080, false, 800), 3_100_000);
    }
}
