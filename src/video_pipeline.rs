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
    host::HostEvent,
    settings::AppConfig,
};

// ─── Encoded frame ────────────────────────────────────────────────────────────

/// Закодированный кадр — единица данных через весь пайплайн.
#[derive(Clone)]
pub struct EncodedFrame {
    pub bytes:    Arc<Vec<u8>>,
    pub is_idr:   bool,
    pub frame_id: u32,
    pub pts_us:   u64,
    /// SPS/PPS — только для IDR кадров.
    pub sps_pps:  Option<Arc<Vec<u8>>>,
    pub width:    u32,
    pub height:   u32,
    pub codec:    &'static str,
}

// ─── Pipeline commands ────────────────────────────────────────────────────────

pub enum PipelineCmd {
    Stop,
    SetFps(u32),
    SetQuality(u32),
    RequestIdr,
    EvrtPeerConnected(SocketAddr),
    EvrtSessionEnded,
}

// ─── Pipeline config ──────────────────────────────────────────────────────────

pub struct PipelineConfig {
    pub app_config:   AppConfig,
    pub peer_id:      String,
    pub events:       Sender<HostEvent>,
    pub relay_stream: std::net::TcpStream,
    pub send_cipher:  Option<crate::crypto::SendCipher>,
    pub recv_cipher:  Option<crate::crypto::RecvCipher>,
    pub evrt_socket:  Option<Arc<UdpSocket>>,
    pub cmd_rx:       Receiver<PipelineCmd>,
    pub peer_msg_rx:  Receiver<crate::rustdesk_proto::PeerMessage>,
}

// ─── run() ───────────────────────────────────────────────────────────────────

pub fn run(cfg: PipelineConfig) {
    let PipelineConfig {
        app_config, peer_id, events,
        relay_stream, send_cipher, recv_cipher: _,
        evrt_socket, cmd_rx, peer_msg_rx: _,
    } = cfg;

    let stop       = Arc::new(AtomicBool::new(false));
    let target_fps = Arc::new(AtomicU32::new(
        app_config.display.target_fps.clamp(5, 60),
    ));
    let quality_ms = Arc::new(AtomicU32::new(1_000));

    // IDR request channel: cmd_rx → encoder
    let (idr_tx, idr_rx) = mpsc::channel::<()>();

    // ── Два канала кадров: encoder → tcp sender, encoder → evrt sender ────────
    // SyncSender с буфером 2: если sender не успевает — encoder притормаживает,
    // а не накапливает очередь (убивает latency).
    let (tcp_tx, tcp_rx)   = mpsc::sync_channel::<EncodedFrame>(2);
    let (evrt_tx, evrt_rx) = mpsc::sync_channel::<EncodedFrame>(2);

    // ── Флаг: EVRT активен? Устанавливается когда клиент прислал punch ────────
    let evrt_active: Arc<Mutex<Option<SocketAddr>>> = Arc::new(Mutex::new(None));

    // ── Encoder + Capture thread ─────────────────────────────────────────────
    {
        let stop_e  = stop.clone();
        let fps_e   = target_fps.clone();
        let qual_e  = quality_ms.clone();
        let cfg_e   = app_config.clone();
        let ev_e    = events.clone();
        let tcp_e   = tcp_tx;
        let evrt_e  = evrt_tx;
        let act_e   = evrt_active.clone();

        thread::Builder::new()
            .name("pipeline-encoder".into())
            .spawn(move || {
                encode_loop(stop_e, fps_e, qual_e, cfg_e, ev_e, tcp_e, evrt_e, act_e, idr_rx);
            })
            .expect("spawn encoder");
    }

    // ── TCP Sender thread ─────────────────────────────────────────────────────
    {
        let stop_t  = stop.clone();
        let ev_t    = events.clone();
        let mut stream  = relay_stream;
        let mut cipher  = send_cipher;
        let act_t   = evrt_active.clone();

        thread::Builder::new()
            .name("pipeline-tcp".into())
            .spawn(move || {
                tcp_send_loop(stop_t, tcp_rx, &mut stream, &mut cipher, act_t, ev_t);
            })
            .expect("spawn tcp-sender");
    }

    // ── EVRT UDP Sender thread ────────────────────────────────────────────────
    if let Some(udp_sock) = evrt_socket {
        let stop_u  = stop.clone();
        let ev_u    = events.clone();
        let cfg_u   = app_config.clone();
        let fps_u   = target_fps.clone();
        let qual_u  = quality_ms.clone();
        let pid_u   = peer_id.clone();
        let act_u   = evrt_active.clone();

        thread::Builder::new()
            .name("pipeline-evrt".into())
            .spawn(move || {
                evrt_send_loop(
                    stop_u, udp_sock, evrt_rx,
                    act_u, ev_u, cfg_u, fps_u, qual_u, pid_u,
                );
            })
            .expect("spawn evrt-sender");
    } else {
        // Нет EVRT сокета — дропаем evrt_rx чтобы encoder не блокировался
        thread::spawn(move || {
            while evrt_rx.recv().is_ok() {}
        });
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

    log(&events, format!("Pipeline для {peer_id} завершён"));
}

// ─── Encode + Capture loop ────────────────────────────────────────────────────

fn encode_loop(
    stop:        Arc<AtomicBool>,
    target_fps:  Arc<AtomicU32>,
    quality_ms:  Arc<AtomicU32>,
    config:      AppConfig,
    events:      Sender<HostEvent>,
    tcp_tx:      SyncSender<EncodedFrame>,
    evrt_tx:     SyncSender<EncodedFrame>,
    evrt_active: Arc<Mutex<Option<SocketAddr>>>,
    idr_rx:      Receiver<()>,
) {
    use crate::host::{
        choose_mf_encoder_codec_pub, encode_mf_frame_pub,
        h264_target_bitrate_bps_pub, ClientVideoSupport,
    };
    use crate::rustdesk_proto::PreferCodec;

    log(&events, "Encoder loop started".into());

    let client_video = ClientVideoSupport {
        h264: true, h265: true, av1: false, prefer: PreferCodec::Auto,
    };
    let desired_codec = choose_mf_encoder_codec_pub(
        config.display.encoder, config.display.codec, client_video,
    );

    let mut mf_enc:     Option<crate::mf_encode::MfVideoEncoder> = None;
    let mut mf_disabled = false;

    // FSR
    let mut fsr = config.display.fsr_quality
        .to_fsr_quality()
        .map(|q| crate::fsr::FsrAdapter::new(crate::fsr::FsrConfig {
            quality: q, sharpness: config.display.fsr_sharpness,
        }));

    // HNS PTS counter
    let mut sample_hns: u64 = 0;
    let hns_per_frame = |fps: u32| 10_000_000u64 / fps.max(1) as u64;

    // Capture double buffer
    type CapSlot = Arc<Mutex<Option<(u32, u32, Vec<u8>)>>>;
    let cap_slot: CapSlot = Arc::new(Mutex::new(None));
    {
        let cap_stop  = stop.clone();
        let cap_fps   = target_fps.clone();
        let cap_slot2 = cap_slot.clone();
        thread::Builder::new()
            .name("pipeline-capture".into())
            .spawn(move || {
                let mut buf = Vec::new();
                loop {
                    if cap_stop.load(Ordering::Relaxed) { break; }
                    let fps = cap_fps.load(Ordering::Relaxed).clamp(5, 60);
                    if let Some((w, h)) = crate::capture::capture_screen_into(&mut buf) {
                        if let Ok(mut slot) = cap_slot2.lock() {
                            match slot.as_mut() {
                                Some(s) if s.0 == w && s.1 == h => {
                                    std::mem::swap(&mut s.2, &mut buf);
                                }
                                _ => *slot = Some((w, h, buf.clone())),
                            }
                        }
                    }
                    let us = 1_000_000u64 / fps.max(1) as u64;
                    thread::sleep(Duration::from_micros(us.saturating_sub(500)));
                }
            })
            .expect("spawn capture");
    }

    let mut frame_id:       u32     = 0;
    let mut last_idr:       Instant = Instant::now();
    let mut next_frame_due: Instant = Instant::now();
    const IDR_MIN: Duration = Duration::from_secs(2);
    const SPIN:    Duration = Duration::from_micros(1_500);

    while !stop.load(Ordering::Relaxed) {
        let fps            = target_fps.load(Ordering::Relaxed).clamp(5, 60);
        let frame_interval = Duration::from_nanos(1_000_000_000 / fps as u64);

        // Точный тайминг (nextFrameDueTicks из EvertyGame)
        let now = Instant::now();
        if now < next_frame_due {
            let wait = next_frame_due - now;
            if wait > SPIN { thread::sleep(wait - SPIN); } else { std::hint::spin_loop(); }
            continue;
        }
        next_frame_due += frame_interval;
        if next_frame_due < Instant::now() {
            next_frame_due = Instant::now() + frame_interval;
        }

        // IDR
        let want_idr = idr_rx.try_recv().is_ok() || last_idr.elapsed() > IDR_MIN;

        // Захват
        let Some((cap_w, cap_h, bgra_raw)) =
            cap_slot.lock().ok().and_then(|mut s| s.take())
        else {
            thread::sleep(Duration::from_millis(1));
            continue;
        };

        // FSR
        let (enc_w, enc_h, bgra_owned);
        let bgra: &[u8] = if let Some(ref mut a) = fsr {
            let (nw, nh) = crate::capture::screen_size().unwrap_or((cap_w, cap_h));
            bgra_owned = a.process_bgra(&bgra_raw, cap_w, cap_h, nw, nh).to_owned();
            enc_w = nw; enc_h = nh;
            &bgra_owned
        } else {
            bgra_owned = Vec::new();
            let _ = &bgra_owned;
            enc_w = cap_w; enc_h = cap_h;
            &bgra_raw
        };

        let quality  = quality_ms.load(Ordering::Relaxed);
        let eff_bps  = h264_target_bitrate_bps_pub(enc_w, enc_h, fps, quality);

        // Кодирование
        let encoded: Option<(Vec<u8>, bool, Option<Vec<u8>>)> =
            if let Some(codec) = desired_codec.filter(|_| !mf_disabled) {
                match encode_mf_frame_pub(
                    &mut mf_enc, codec, enc_w, enc_h, fps, eff_bps, bgra, want_idr,
                ) {
                    Ok(Some(pkt)) => {
                        let sps = if pkt.key {
                            mf_enc.as_ref().and_then(|e| e.codec_config())
                        } else {
                            None
                        };
                        Some((pkt.bytes, pkt.key, sps))
                    }
                    Ok(None) => None,
                    Err(e) => {
                        log(&events, format!("Encode error: {e} — MF disabled"));
                        mf_disabled = true;
                        None
                    }
                }
            } else if want_idr {
                Some((encode_png(&bgra_raw, cap_w, cap_h), true, None))
            } else {
                None
            };

        let Some((bytes, is_idr, sps_pps)) = encoded else { continue };

        frame_id = frame_id.wrapping_add(1);
        let pts_us = sample_hns / 10;
        sample_hns = sample_hns.wrapping_add(hns_per_frame(fps));

        if is_idr { last_idr = Instant::now(); }

        let codec_label: &'static str = match desired_codec {
            Some(c) if c == crate::nvenc::NvencCodec::H265 => "H265",
            _ => "H264",
        };

        let frame = EncodedFrame {
            bytes:    Arc::new(bytes),
            is_idr,
            frame_id,
            pts_us,
            sps_pps:  sps_pps.map(Arc::new),
            width:    enc_w,
            height:   enc_h,
            codec:    codec_label,
        };

        // ── Dispatch ──────────────────────────────────────────────────────────
        // EVRT активен: все кадры → EVRT, только IDR → TCP (синхронизация)
        // EVRT не активен: все кадры → TCP
        let evrt_on = evrt_active.lock().map(|g| g.is_some()).unwrap_or(false);

        if evrt_on {
            // EVRT primary: шлём все кадры в EVRT, IDR в TCP для sync
            match evrt_tx.try_send(frame.clone()) {
                Ok(()) => {}
                Err(mpsc::TrySendError::Full(_)) => {
                    // EVRT буфер полон — клиент не успевает, пропускаем
                }
                Err(mpsc::TrySendError::Disconnected(_)) => break,
            }
            if is_idr {
                // IDR в TCP для того чтобы клиент мог переключиться обратно
                match tcp_tx.try_send(frame) {
                    Ok(()) | Err(mpsc::TrySendError::Full(_)) => {}
                    Err(mpsc::TrySendError::Disconnected(_)) => break,
                }
            }
        } else {
            // TCP primary: все кадры в TCP
            match tcp_tx.try_send(frame) {
                Ok(()) => {}
                Err(mpsc::TrySendError::Full(_)) => {}
                Err(mpsc::TrySendError::Disconnected(_)) => break,
            }
        }
    }

    stop.store(true, Ordering::Relaxed);
    log(&events, "Encoder loop stopped".into());
}

// ─── TCP Sender loop ──────────────────────────────────────────────────────────

fn tcp_send_loop(
    stop:        Arc<AtomicBool>,
    frame_rx:    Receiver<EncodedFrame>,
    stream:      &mut std::net::TcpStream,
    cipher:      &mut Option<crate::crypto::SendCipher>,
    evrt_active: Arc<Mutex<Option<SocketAddr>>>,
    events:      Sender<HostEvent>,
) {
    // Write timeout чтобы не висеть при разрыве соединения
    stream.set_write_timeout(Some(Duration::from_secs(2))).ok();
    log(&events, "TCP sender started".into());

    while !stop.load(Ordering::Relaxed) {
        let frame = match frame_rx.recv_timeout(Duration::from_millis(50)) {
            Ok(f) => f,
            Err(mpsc::RecvTimeoutError::Timeout) => continue,
            Err(_) => break,
        };

        let video_msg = make_tcp_video_frame(&frame);
        let mut payload = crate::transport::encode_peer_message_raw(&video_msg);
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
        peer_message, video_frame, EncodedVideoFrame, EncodedVideoFrames,
        PeerMessage, VideoFrame,
    };
    let encoded = EncodedVideoFrame {
        data: (*frame.bytes).clone(),
        key:  frame.is_idr,
        pts:  frame.pts_us as i64,
    };
    let frames = EncodedVideoFrames { frames: vec![encoded] };
    let union = match frame.codec {
        "H265" => video_frame::Union::H265s(frames),
        _      => video_frame::Union::H264s(frames),
    };
    PeerMessage {
        union: Some(peer_message::Union::VideoFrame(VideoFrame {
            union: Some(union), display: 0,
        })),
    }
}

// ─── EVRT UDP Sender loop ─────────────────────────────────────────────────────

fn evrt_send_loop(
    stop:        Arc<AtomicBool>,
    socket:      Arc<UdpSocket>,
    frame_rx:    Receiver<EncodedFrame>,
    evrt_active: Arc<Mutex<Option<SocketAddr>>>,
    events:      Sender<HostEvent>,
    config:      AppConfig,
    target_fps:  Arc<AtomicU32>,
    quality_ms:  Arc<AtomicU32>,
    peer_id:     String,
) {
    log(&events, "EVRT UDP sender: ожидание punch…".into());

    // Ждём punch от клиента
    let deadline = Instant::now() + Duration::from_secs(8);
    let mut buf  = vec![0u8; crate::evrt::MAX_PACKET_SIZE + 64];
    socket.set_read_timeout(Some(Duration::from_millis(300))).ok();

    let peer_addr = loop {
        if stop.load(Ordering::Relaxed) || Instant::now() > deadline {
            log(&events, "EVRT: punch timeout — UDP сессия не запущена".into());
            // Дренируем evrt_rx чтобы encoder не блокировался
            while frame_rx.recv_timeout(Duration::from_millis(10)).is_ok() {}
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
        socket:        socket.clone(),
        config:        config.clone(),
        peer_id:       peer_id.clone(),
        events:        events.clone(),
        stop:          stop.clone(),
        frame_rx,          // ← готовые кадры из encoder, не своя запись
        target_fps,
        quality_milli: quality_ms,
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

// ─── helpers ─────────────────────────────────────────────────────────────────

fn encode_png(bgra: &[u8], w: u32, h: u32) -> Vec<u8> {
    use image::{ImageBuffer, Rgba};
    let img: ImageBuffer<Rgba<u8>, Vec<u8>> =
        ImageBuffer::from_raw(w, h, bgra.to_vec())
            .unwrap_or_else(|| ImageBuffer::new(w, h));
    let mut out = Vec::new();
    img.write_to(&mut std::io::Cursor::new(&mut out), image::ImageFormat::Png).ok();
    out
}

fn log(events: &Sender<HostEvent>, msg: String) {
    eprintln!("[pipeline] {msg}");
    let _ = events.send(HostEvent::Log(msg));
}
