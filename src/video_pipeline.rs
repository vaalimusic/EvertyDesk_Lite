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
    collections::{HashMap, HashSet},
    net::{SocketAddr, UdpSocket},
    sync::{
        atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering},
        mpsc::{self, Receiver, Sender, SyncSender},
        Arc, Mutex,
    },
    thread,
    time::{Duration, Instant},
};

use crate::{
    host::{ClientVideoSupport, HostEvent},
    rustdesk_proto::PreferCodec,
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

#[allow(dead_code)] // Reserved for client-side IDR requests and EVRT lifecycle wiring.
pub enum PipelineCmd {
    Stop,
    SetFps(u32),
    SetQuality(u32),
    SetDisplay(i32),
    SetSubscribedDisplays(Vec<i32>),
    AddSubscribedDisplays(Vec<i32>),
    RemoveSubscribedDisplays(Vec<i32>),
    RefreshDisplay(i32),
    SendSwitchDisplay(i32),
    RequestIdr,
    EvrtPeerConnected(SocketAddr),
    EvrtSessionEnded,
    /// Hot-switch encoder codec: restarts video services with the new preference.
    SetClientCodec(crate::host::ClientVideoSupport),
    /// EVRT2CKMAX-TASK-01: last known remote cursor position (host screen pixel
    /// coordinates), driven by incoming MouseEvent. Used as the Visible Region
    /// focus point for EVRTCK's priority tile ordering — see
    /// `EvrtckEncoder::set_focus_pixel`. Experimental: only affects sessions
    /// where `want_evrtck` is true (client opted into EVRTCK).
    CursorMoved {
        x: u32,
        y: u32,
    },
}

struct VideoServiceHandle {
    stop: Arc<AtomicBool>,
    idr_tx: Sender<()>,
    handle: thread::JoinHandle<()>,
    client_video: ClientVideoSupport,
    using_evrtck: bool,
}

// ─── Pipeline config ──────────────────────────────────────────────────────────

pub struct PipelineConfig {
    pub app_config: AppConfig,
    pub peer_id: String,
    pub events: Sender<HostEvent>,
    pub client_video: ClientVideoSupport,
    pub initial_target_fps: u32,
    pub initial_quality_milli: u32,
    pub relay_stream: std::net::TcpStream,
    /// Шифрование исходящего TCP потока (видео).
    /// RecvCipher остаётся в relay_session_inner для расшифровки управляющих сообщений.
    pub send_cipher: Option<crate::crypto::SendCipher>,
    pub evrt_socket: Option<Arc<UdpSocket>>,
    pub evrt_token: Option<String>,
    pub cmd_rx: Receiver<PipelineCmd>,
    pub peer_msg_rx: Receiver<crate::rustdesk_proto::PeerMessage>,
}

// ─── run() ───────────────────────────────────────────────────────────────────

pub fn run(cfg: PipelineConfig) {
    let PipelineConfig {
        app_config,
        peer_id,
        events,
        client_video: initial_client_video,
        initial_target_fps,
        initial_quality_milli,
        relay_stream,
        send_cipher,
        evrt_socket,
        evrt_token,
        cmd_rx,
        peer_msg_rx,
    } = cfg;

    let stop = Arc::new(AtomicBool::new(false));
    let target_fps = Arc::new(AtomicU32::new(initial_target_fps.clamp(5, 60)));
    let quality_ms = Arc::new(AtomicU32::new(initial_quality_milli.max(1)));
    let bitrate_scale_milli = Arc::new(AtomicU32::new(1_000));
    let mut client_video = initial_client_video;

    // want_evrtck: фиксируется по первому SetClientCodec от клиента.
    // Auto/VP9 → клиент поддерживает EVRTCK. H265/H264/AV1 → стандартный кодек.
    // Последующие смены (TCP relay churn) не меняют этот флаг — он отражает
    // истинную EVRTCK-совместимость клиента, а не текущий TCP codec.
    let want_evrtck: Arc<AtomicBool> = Arc::new(AtomicBool::new(matches!(
        initial_client_video.prefer,
        PreferCodec::Auto | PreferCodec::Vp9
    )));
    let mut first_codec_set = false;
    // Game-mode codec negotiation state.
    // In game mode (want_evrtck=false), SetClientCodec messages may arrive before
    // EVRT is established, causing spurious H264 fallbacks and pipeline restarts.
    // We buffer the desired codec here and commit exactly once on EVRT punch.
    let mut pending_game_video: ClientVideoSupport = initial_client_video;
    let mut game_codec_committed = false;
    // If EVRT punch never arrives (e.g. Android relay-only), commit pending codec at deadline.
    // 3s gives local LAN EVRT enough time to punch while keeping TCP relay startup fast.
    let evrt_punch_deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);

    // IDR request channel: cmd_rx → encoder
    let (global_idr_tx, global_idr_rx) = mpsc::channel::<()>();

    // ── TCP канал несёт И видео, И control (shell output), И (при EVRT timeout)
    //    аудио-зеркало (TcpAudioFrame, ~100 маленьких кадров/сек, см. audio-mirror
    //    ниже) ────────────────────────────────────────────────────────────────
    // TcpItem::Video — видеокадры (приоритет latency)
    // TcpItem::Peer  — shell output, control-сообщения, TCP-аудио
    // Буфер поднят с 4 до 16: при 4 слотах TcpItem::Peer(audio) отправляется через
    // try_send и почти всегда проигрывал гонку за место видеокадрам — звук
    // тихо терялся целиком на relay-only сессиях (WAN без EVRT punch). 16 слотов
    // при ~160 сообщений/сек совокупно даёт ≤100мс худшего буферизования —
    // не свободный обед, но предпочтительнее "звука нет вообще".
    let (tcp_tx, tcp_rx) = mpsc::sync_channel::<TcpItem>(16);
    let (evrt_tx, evrt_rx) = mpsc::sync_channel::<EncodedFrame>(2);
    let mut worker_handles = Vec::new();
    let mut video_services: HashMap<i32, VideoServiceHandle> = HashMap::new();
    let mut subscribed_displays: HashSet<i32> = HashSet::new();

    // ── Флаг: EVRT активен? Устанавливается когда клиент прислал punch ────────
    let evrt_active: Arc<Mutex<Option<SocketAddr>>> = Arc::new(Mutex::new(None));

    // ── Максимальное разрешение клиента — encoder downscales до этого ──────────
    // Заполняется EVRT сессией из ReceiverFeedback.max_width/max_height.
    let client_max_res: Arc<AtomicU64> = Arc::new(AtomicU64::new(0));
    // Фактическое разрешение кодирования (после downscale), упакованное w<<32|h.
    // Обновляется encode_loop каждый кадр; EVRT сессия читает для корректного bitrate.
    let actual_encode_res: Arc<AtomicU64> = Arc::new(AtomicU64::new(0));

    // ── EVRT2CKMAX-TASK-01: последняя известная позиция курсора клиента ────────
    // Упаковано x<<32|y, в пиксельных координатах экрана хоста. 0 = ещё не
    // известно (encode_loop тогда не выставляет фокус — обычный raster order).
    // Обновляется командой PipelineCmd::CursorMoved из handle_client_input_pipeline
    // при каждом MouseEvent. Читается encode_loop перед EVRTCK-кодированием.
    let cursor_pos: Arc<AtomicU64> = Arc::new(AtomicU64::new(0));
    let evrtck_silicon_requested: Arc<AtomicBool> = Arc::new(AtomicBool::new(false));
    let evrtck_return_requested: Arc<AtomicBool> = Arc::new(AtomicBool::new(false));
    let evrtck_scheduler_silicon_active: Arc<AtomicBool> = Arc::new(AtomicBool::new(false));

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
    set_subscribed_displays(
        vec![0],
        &mut video_services,
        &mut subscribed_displays,
        &tcp_tx,
        &evrt_tx,
        &stop,
        &target_fps,
        &quality_ms,
        &bitrate_scale_milli,
        &app_config,
        client_video,
        &events,
        &evrt_active,
        &client_max_res,
        &actual_encode_res,
        &want_evrtck,
        &cursor_pos,
        &evrtck_silicon_requested,
        &evrtck_return_requested,
        &evrtck_scheduler_silicon_active,
    );

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
        // `evrt_session` builds the very first `SessionConfig` — the codec name
        // the client sees BEFORE any frame arrives, and therefore the decoder
        // it opens first — out of `config.display.codec`. Feed it the codec the
        // CLIENT asked for, not this host's own preference setting, which has
        // nothing to do with what will actually be sent down this session.
        //
        // Getting this wrong is not cosmetic: announcing a codec that doesn't
        // match the bytes is what crashed the client outright (task #39 —
        // `HEVCDECODER_STORE.dll` 0xC0000005, announced H265 while sending
        // H264). The host's own `SetClientCodec`/`TYPE_CODEC_CONFIG` mechanism
        // does correct a mismatch on the first frame, but that correction
        // travels a DIFFERENT path (mpsc channel) than the frames themselves
        // (packet queue), so there is a real, if narrow, window where frames
        // can reach a decoder that hasn't switched yet. Announcing the client's
        // own request closes that window for the common case — the client asked
        // for H265 and the host has an H265 encoder, so no switch is needed at
        // all.
        let cfg_u = {
            use crate::settings::CodecPreference;
            let mut cfg = app_config.clone();
            // Кодек, который клиент ПРОСИТ. Это ещё не ответ на вопрос, что он
            // получит: у хоста может не быть соответствующего энкодера.
            let requested = match initial_client_video.prefer {
                // Auto/VP9 — ровно то условие, по которому хост уходит в EVRTCK
                // (см. `want_evrtck` выше). Держим синхронно.
                PreferCodec::Auto | PreferCodec::Vp9 => CodecPreference::Evrtck,
                PreferCodec::H264 => CodecPreference::H264,
                PreferCodec::H265 => CodecPreference::H265,
                PreferCodec::Av1 => CodecPreference::Av1,
                // VP8 наш `preferred_codec()` не производит никогда, и VP8-энкодера
                // у хоста нет — каскад сядет на свою H264-сетку.
                PreferCodec::Vp8 => CodecPreference::H264,
            };
            // А теперь — что реально будет закодировано. Объявлять клиенту
            // нужно ИМЕННО ЭТО: `SessionConfig.codec` решает, какой декодер он
            // откроет ещё до первого кадра, и расхождение убивает его процесс
            // (см. doc у `negotiated_session_codec`). Живо поймано: клиент
            // просил H265 и умеет его декодировать, но у хоста в MF только
            // H264(hw) — каскад отдавал H264 под вывеской H265, и клиент падал
            // сразу после подключения.
            cfg.display.codec = if requested == CodecPreference::Evrtck {
                CodecPreference::Evrtck
            } else {
                match crate::host::negotiated_session_codec(
                    app_config.display.encoder,
                    requested,
                    initial_client_video,
                ) {
                    Some(crate::nvenc::NvencCodec::H265) => CodecPreference::H265,
                    Some(crate::nvenc::NvencCodec::Av1) => CodecPreference::Av1,
                    // H264 либо `None` (аппаратного бэкенда нет вообще →
                    // программный OpenH264, на проводе всё равно H264).
                    _ => CodecPreference::H264,
                }
            };
            cfg
        };
        let fps_u = target_fps.clone();
        let qual_u = quality_ms.clone();
        let bitrate_scale_u = bitrate_scale_milli.clone();
        let idr_u = global_idr_tx.clone();
        let pid_u = peer_id.clone();
        let act_u = evrt_active.clone();
        let res_u = client_max_res.clone();
        let enc_res_u = actual_encode_res.clone();
        let want_evrtck_u = want_evrtck.clone();

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
                    idr_u,
                    pid_u,
                    evrt_token,
                    res_u,
                    enc_res_u,
                    want_evrtck_u,
                );
            })
            .expect("spawn evrt-sender");
        worker_handles.push(handle);
    } else {
        // Нет EVRT сокета — дропаем evrt_rx чтобы encoder не блокировался
        worker_handles.push(thread::spawn(move || while evrt_rx.recv().is_ok() {}));
    }

    // ── Command loop (этот тред) ───────────────────────────────────────────────
    // Последнее известное состояние EVRT: отслеживаем изменения через polling
    // (evrt_send_loop не имеет cmd_tx и устанавливает evrt_active напрямую).
    let mut prev_evrt_addr: Option<SocketAddr> = None;

    while !stop.load(Ordering::Relaxed) {
        while let Ok(()) = global_idr_rx.try_recv() {
            for service in video_services.values() {
                let _ = service.idr_tx.send(());
            }
        }

        // Polling EVRT state: детектируем переходы None→Some и Some→None.
        // Срабатывает максимум через 50ms после реального события (recv_timeout).
        let cur_evrt = evrt_active.lock().ok().and_then(|g| *g);
        if cur_evrt != prev_evrt_addr {
            prev_evrt_addr = cur_evrt;
            if let Some(addr) = cur_evrt {
                let _ = global_idr_tx.send(());
                let mode = if want_evrtck.load(Ordering::Relaxed) {
                    "EVRTCK"
                } else {
                    "H265/EVRT"
                };
                log(
                    &events,
                    format!("Pipeline: EVRT активен → {addr} (режим={mode})"),
                );

                // Game-mode codec commit: runs exactly once on first EVRT punch.
                // All SetClientCodec messages received before this point were buffered
                // in pending_game_video without restarting the pipeline. Now we apply
                // the final codec choice with a single controlled restart.
                let is_game_mode = !want_evrtck.load(Ordering::Relaxed);
                if is_game_mode && !game_codec_committed {
                    game_codec_committed = true;
                    // Pick the best codec the client actually supports, ignoring
                    // the order of SetClientCodec messages. The client may send
                    // H265 then fall back to H264 (no IDR received yet), so the
                    // last message is unreliable.
                    //
                    // Respect the client's ACTUAL selection first (e.g. AV1, now
                    // that NVENC AV1 is wired up — see nvenc_shim.cpp codec_guid).
                    // Previously this unconditionally forced H265 whenever
                    // h265==true, silently overriding an AV1 pick even when the
                    // host could genuinely encode it — the codec selector in the
                    // UI had no real effect. Only fall back to "prefer H265" when
                    // the client's own choice isn't actually supported.
                    let best_prefer = if pending_game_video.prefer == PreferCodec::Av1
                        && pending_game_video.av1
                    {
                        PreferCodec::Av1
                    } else if pending_game_video.prefer == PreferCodec::H265
                        && pending_game_video.h265
                    {
                        PreferCodec::H265
                    } else if pending_game_video.prefer == PreferCodec::H264
                        && pending_game_video.h264
                    {
                        PreferCodec::H264
                    } else if pending_game_video.h265 {
                        PreferCodec::H265
                    } else {
                        pending_game_video.prefer
                    };
                    let mut committed = pending_game_video;
                    committed.prefer = best_prefer;
                    let target_prefer = best_prefer;
                    client_video = committed;
                    let displays: Vec<i32> = subscribed_displays.iter().copied().collect();
                    if !displays.is_empty() {
                        log(
                            &events,
                            format!("Pipeline: game codec commit → {:?}", target_prefer),
                        );
                        set_subscribed_displays(
                            displays,
                            &mut video_services,
                            &mut subscribed_displays,
                            &tcp_tx,
                            &evrt_tx,
                            &stop,
                            &target_fps,
                            &quality_ms,
                            &bitrate_scale_milli,
                            &app_config,
                            client_video,
                            &events,
                            &evrt_active,
                            &client_max_res,
                            &actual_encode_res,
                            &want_evrtck,
                            &cursor_pos,
                            &evrtck_silicon_requested,
                            &evrtck_return_requested,
                            &evrtck_scheduler_silicon_active,
                        );
                    }
                }
            } else {
                log(&events, "Pipeline: EVRT завершён, TCP relay primary".into());
            }
        }

        // Fallback: если EVRT punch не пришёл за 3 сек, коммитим pending game codec.
        // Актуально для клиентов за NAT где UDP punch недоступен (Android relay-only).
        //
        // ВАЖНО: этот путь по определению означает TCP relay-only (EVRT UDP не
        // поднялся) — H265/AV1 по relay ломает MediaCodec decode на Android (тот
        // же guard, что и в обычном не-game-mode пути ниже, "H265/AV1 over TCP
        // relay causes MediaCodec decode errors on Android"). До фикса capability
        // (h265_available()/av1_available() были хардкожены false на Android) это
        // маскировалось — клиент никогда не репортил h265=true, поэтому сюда всегда
        // попадал H264. После фикса client_video.h265 стал честным, и эта ветка
        // начала реально выбирать H265 для relay-only сессии → пустой экран.
        let is_game_mode = !want_evrtck.load(Ordering::Relaxed);
        if is_game_mode && !game_codec_committed && std::time::Instant::now() >= evrt_punch_deadline
        {
            game_codec_committed = true;
            let best_prefer = PreferCodec::H264;
            let mut committed = pending_game_video;
            committed.prefer = best_prefer;
            client_video = committed;
            let displays: Vec<i32> = subscribed_displays.iter().copied().collect();
            if !displays.is_empty() {
                log(
                    &events,
                    format!(
                        "Pipeline: game codec commit (EVRT timeout) → {:?}",
                        best_prefer
                    ),
                );
                set_subscribed_displays(
                    displays,
                    &mut video_services,
                    &mut subscribed_displays,
                    &tcp_tx,
                    &evrt_tx,
                    &stop,
                    &target_fps,
                    &quality_ms,
                    &bitrate_scale_milli,
                    &app_config,
                    client_video,
                    &events,
                    &evrt_active,
                    &client_max_res,
                    &actual_encode_res,
                    &want_evrtck,
                    &cursor_pos,
                    &evrtck_silicon_requested,
                    &evrtck_return_requested,
                    &evrtck_scheduler_silicon_active,
                );
            }

            // ★ Аудио по TCP relay: этот момент по определению означает, что EVRT
            // UDP никогда не поднимется для данной сессии — значит EVRT Audio
            // (evrt_session.rs) тоже никогда не запустится, звука не будет вообще.
            // Зеркалим WASAPI-захват в TCP через тот же evrt_audio::run_audio_capture,
            // что и EVRT-путь, но с фиктивным UDP-адресом (реальная доставка идёт
            // через колбэк on_tcp_frame → tcp_tx). WASAPI-логика захвата не тронута.
            {
                use crate::rustdesk_proto::{misc, peer_message, Misc, PeerMessage};
                let tcp_tx_audio = tcp_tx.clone();
                let audio_stop = stop.clone();
                let audio_events = events.clone();
                let audio_events_log = events.clone();
                thread::Builder::new()
                    .name("tcp-audio-mirror".into())
                    .spawn(move || {
                        // Фиктивные socket/peer_addr: run_audio_capture требует их для
                        // неизменённого EVRT UDP пути, но здесь этот путь просто не
                        // доставит пакеты никуда (не тронут, не наблюдаем ошибок —
                        // `let _ = socket.send_to(...)` в evrt_audio.rs их и так глушит).
                        let Ok(dummy_sock) = std::net::UdpSocket::bind("0.0.0.0:0") else {
                            return;
                        };
                        let dummy_peer: SocketAddr = "127.0.0.1:1".parse().unwrap();
                        let sent_count = std::sync::atomic::AtomicU64::new(0);
                        let dropped_count = std::sync::atomic::AtomicU64::new(0);
                        let on_tcp_frame: Box<dyn Fn(&[u8]) + Send> =
                            Box::new(move |chunk: &[u8]| {
                                let msg = PeerMessage {
                                    union: Some(peer_message::Union::Misc(Misc {
                                        union: Some(misc::Union::TcpAudioFrame(chunk.to_vec())),
                                    })),
                                };
                                match tcp_tx_audio.try_send(TcpItem::Peer(msg)) {
                                    Ok(()) => {
                                        let n = sent_count
                                            .fetch_add(1, std::sync::atomic::Ordering::Relaxed)
                                            + 1;
                                        if n == 1 || n % 50 == 0 {
                                            log(
                                                &audio_events_log,
                                                format!(
                                                    "TCP audio mirror: #{n} sent ok, {} bytes",
                                                    chunk.len()
                                                ),
                                            );
                                        }
                                    }
                                    Err(_) => {
                                        let d = dropped_count
                                            .fetch_add(1, std::sync::atomic::Ordering::Relaxed)
                                            + 1;
                                        if d == 1 || d % 50 == 0 {
                                            log(
                                                &audio_events_log,
                                                format!(
                                                    "TCP audio mirror: #{d} DROPPED (channel full)"
                                                ),
                                            );
                                        }
                                    }
                                }
                            });
                        crate::evrt_audio::run_audio_capture(
                            Arc::new(dummy_sock),
                            dummy_peer,
                            audio_stop,
                            None,
                            audio_events,
                            Some(on_tcp_frame),
                        );
                    })
                    .ok();
            }
        }

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
            Ok(PipelineCmd::CursorMoved { x, y }) => {
                cursor_pos.store(((x as u64) << 32) | (y as u64), Ordering::Relaxed);
            }
            Ok(PipelineCmd::SetDisplay(display)) => {
                let display = display.max(0);
                set_subscribed_displays(
                    vec![display],
                    &mut video_services,
                    &mut subscribed_displays,
                    &tcp_tx,
                    &evrt_tx,
                    &stop,
                    &target_fps,
                    &quality_ms,
                    &bitrate_scale_milli,
                    &app_config,
                    client_video,
                    &events,
                    &evrt_active,
                    &client_max_res,
                    &actual_encode_res,
                    &want_evrtck,
                    &cursor_pos,
                    &evrtck_silicon_requested,
                    &evrtck_return_requested,
                    &evrtck_scheduler_silicon_active,
                );
            }
            Ok(PipelineCmd::SetSubscribedDisplays(displays)) => {
                set_subscribed_displays(
                    displays,
                    &mut video_services,
                    &mut subscribed_displays,
                    &tcp_tx,
                    &evrt_tx,
                    &stop,
                    &target_fps,
                    &quality_ms,
                    &bitrate_scale_milli,
                    &app_config,
                    client_video,
                    &events,
                    &evrt_active,
                    &client_max_res,
                    &actual_encode_res,
                    &want_evrtck,
                    &cursor_pos,
                    &evrtck_silicon_requested,
                    &evrtck_return_requested,
                    &evrtck_scheduler_silicon_active,
                );
            }
            Ok(PipelineCmd::AddSubscribedDisplays(displays)) => {
                let mut next: Vec<i32> = subscribed_displays.iter().copied().collect();
                next.extend(displays.into_iter().map(|display| display.max(0)));
                set_subscribed_displays(
                    next,
                    &mut video_services,
                    &mut subscribed_displays,
                    &tcp_tx,
                    &evrt_tx,
                    &stop,
                    &target_fps,
                    &quality_ms,
                    &bitrate_scale_milli,
                    &app_config,
                    client_video,
                    &events,
                    &evrt_active,
                    &client_max_res,
                    &actual_encode_res,
                    &want_evrtck,
                    &cursor_pos,
                    &evrtck_silicon_requested,
                    &evrtck_return_requested,
                    &evrtck_scheduler_silicon_active,
                );
            }
            Ok(PipelineCmd::RemoveSubscribedDisplays(displays)) => {
                let removed: HashSet<i32> =
                    displays.into_iter().map(|display| display.max(0)).collect();
                let next: Vec<i32> = subscribed_displays
                    .iter()
                    .filter(|display| !removed.contains(display))
                    .copied()
                    .collect();
                set_subscribed_displays(
                    next,
                    &mut video_services,
                    &mut subscribed_displays,
                    &tcp_tx,
                    &evrt_tx,
                    &stop,
                    &target_fps,
                    &quality_ms,
                    &bitrate_scale_milli,
                    &app_config,
                    client_video,
                    &events,
                    &evrt_active,
                    &client_max_res,
                    &actual_encode_res,
                    &want_evrtck,
                    &cursor_pos,
                    &evrtck_silicon_requested,
                    &evrtck_return_requested,
                    &evrtck_scheduler_silicon_active,
                );
            }
            Ok(PipelineCmd::RefreshDisplay(display)) => {
                let display = display.max(0);
                if let Some(service) = video_services.get(&display) {
                    let _ = service.idr_tx.send(());
                }
                send_ordered_switch_display(&tcp_tx, &stop, display);
            }
            Ok(PipelineCmd::SendSwitchDisplay(display)) => {
                send_ordered_switch_display(&tcp_tx, &stop, display.max(0));
            }
            Ok(PipelineCmd::RequestIdr) => {
                let _ = global_idr_tx.send(());
            }
            Ok(PipelineCmd::EvrtPeerConnected(_addr)) => {
                // Polling в начале цикла детектирует это изменение через evrt_active Arc.
                // Этот вариант никогда не присылается (evrt_send_loop не имеет cmd_tx),
                // оставлен для будущей совместимости.
            }
            Ok(PipelineCmd::EvrtSessionEnded) => {
                // Аналогично — polling детектирует evrt_active → None.
            }
            Ok(PipelineCmd::SetClientCodec(new_video)) => {
                // First message permanently determines EVRTCK vs game-mode.
                if !first_codec_set {
                    first_codec_set = true;
                    want_evrtck.store(
                        matches!(new_video.prefer, PreferCodec::Auto | PreferCodec::Vp9),
                        Ordering::Relaxed,
                    );
                }
                let is_game_mode = !want_evrtck.load(Ordering::Relaxed);
                if is_game_mode {
                    // Game mode: buffer the client's desired codec; never restart pipeline
                    // here. The codec is committed exactly once when EVRT punch arrives
                    // (see polling block above). After commit, late changes are ignored to
                    // prevent decoder churn during an active gaming session.
                    if !game_codec_committed {
                        pending_game_video = new_video;
                        log(
                            &events,
                            format!(
                                "Pipeline: game codec pending → {:?} (await EVRT punch)",
                                new_video.prefer
                            ),
                        );
                    } else {
                        // Codec locked — send forced IDR so client can resume decoding.
                        let _ = global_idr_tx.send(());
                        log(
                            &events,
                            format!(
                                "Pipeline: game codec locked ({:?}), IDR forced for late SetClientCodec({:?})",
                                client_video.prefer, new_video.prefer
                            ),
                        );
                    }
                    // Always update capability flags so negotiation sees current support.
                    client_video.h264 = new_video.h264;
                    client_video.h265 = new_video.h265;
                    client_video.av1 = new_video.av1;
                } else {
                    // Normal EVRTCK mode: existing hot-switch behavior.
                    // H265/AV1 over TCP relay causes MediaCodec decode errors on Android;
                    // fall back to H264 until EVRT is established.
                    let mut eff_video = new_video;
                    let evrt_live = evrt_active.lock().map(|g| g.is_some()).unwrap_or(false);
                    if !evrt_live
                        && matches!(eff_video.prefer, PreferCodec::H265 | PreferCodec::Av1)
                    {
                        eff_video.prefer = PreferCodec::H264;
                    }
                    if eff_video.prefer == client_video.prefer {
                        client_video = eff_video;
                    } else {
                        client_video = eff_video;
                        let displays: Vec<i32> = subscribed_displays.iter().copied().collect();
                        log(
                            &events,
                            format!("Pipeline: кодек переключён → {:?}", eff_video.prefer),
                        );
                        set_subscribed_displays(
                            displays,
                            &mut video_services,
                            &mut subscribed_displays,
                            &tcp_tx,
                            &evrt_tx,
                            &stop,
                            &target_fps,
                            &quality_ms,
                            &bitrate_scale_milli,
                            &app_config,
                            client_video,
                            &events,
                            &evrt_active,
                            &client_max_res,
                            &actual_encode_res,
                            &want_evrtck,
                            &cursor_pos,
                            &evrtck_silicon_requested,
                            &evrtck_return_requested,
                            &evrtck_scheduler_silicon_active,
                        );
                    }
                }
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {
                if evrtck_silicon_requested.swap(false, Ordering::Relaxed)
                    && want_evrtck.load(Ordering::Relaxed)
                {
                    let evrt_live = evrt_active.lock().map(|g| g.is_some()).unwrap_or(false);
                    if let Some(prefer) = evrtck_silicon_prefer(client_video, evrt_live) {
                        want_evrtck.store(false, Ordering::Relaxed);
                        evrtck_scheduler_silicon_active.store(true, Ordering::Relaxed);
                        client_video.prefer = prefer;
                        let displays: Vec<i32> = subscribed_displays.iter().copied().collect();
                        log(
                            &events,
                            format!(
                                "EVRTCK scheduler: switching to silicon codec {:?} (evrt_live={})",
                                prefer, evrt_live
                            ),
                        );
                        set_subscribed_displays(
                            displays,
                            &mut video_services,
                            &mut subscribed_displays,
                            &tcp_tx,
                            &evrt_tx,
                            &stop,
                            &target_fps,
                            &quality_ms,
                            &bitrate_scale_milli,
                            &app_config,
                            client_video,
                            &events,
                            &evrt_active,
                            &client_max_res,
                            &actual_encode_res,
                            &want_evrtck,
                            &cursor_pos,
                            &evrtck_silicon_requested,
                            &evrtck_return_requested,
                            &evrtck_scheduler_silicon_active,
                        );
                        let _ = global_idr_tx.send(());
                    } else {
                        log(
                            &events,
                            "EVRTCK scheduler: silicon candidate ignored; no compatible client codec"
                                .to_owned(),
                        );
                    }
                }
                if evrtck_return_requested.swap(false, Ordering::Relaxed)
                    && evrtck_scheduler_silicon_active.load(Ordering::Relaxed)
                {
                    evrtck_scheduler_silicon_active.store(false, Ordering::Relaxed);
                    want_evrtck.store(true, Ordering::Relaxed);
                    client_video.prefer = PreferCodec::Auto;
                    let displays: Vec<i32> = subscribed_displays.iter().copied().collect();
                    log(
                        &events,
                        "EVRTCK scheduler: returning to EVRTCK after stable low-delta period"
                            .to_owned(),
                    );
                    set_subscribed_displays(
                        displays,
                        &mut video_services,
                        &mut subscribed_displays,
                        &tcp_tx,
                        &evrt_tx,
                        &stop,
                        &target_fps,
                        &quality_ms,
                        &bitrate_scale_milli,
                        &app_config,
                        client_video,
                        &events,
                        &evrt_active,
                        &client_max_res,
                        &actual_encode_res,
                        &want_evrtck,
                        &cursor_pos,
                        &evrtck_silicon_requested,
                        &evrtck_return_requested,
                        &evrtck_scheduler_silicon_active,
                    );
                    let _ = global_idr_tx.send(());
                }
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                stop.store(true, Ordering::Relaxed);
                break;
            }
        }
    }

    stop.store(true, Ordering::Relaxed);
    for service in video_services.values() {
        service.stop.store(true, Ordering::Relaxed);
    }
    // Join workers inline so run() only returns after capture threads are fully
    // stopped. The previous detach approach left DXGI capture running across
    // sessions, causing hangs on reconnect and leaked threads that blocked process exit.
    for (_, service) in video_services {
        let _ = service.handle.join();
    }
    for handle in worker_handles {
        let _ = handle.join();
    }
    log(&events, format!("Pipeline для {peer_id} завершён"));
}

// ─── Encode + Capture loop ────────────────────────────────────────────────────

fn set_subscribed_displays(
    displays: Vec<i32>,
    services: &mut HashMap<i32, VideoServiceHandle>,
    subscribed: &mut HashSet<i32>,
    tcp_tx: &SyncSender<TcpItem>,
    evrt_tx: &SyncSender<EncodedFrame>,
    pipeline_stop: &Arc<AtomicBool>,
    target_fps: &Arc<AtomicU32>,
    quality_ms: &Arc<AtomicU32>,
    bitrate_scale_milli: &Arc<AtomicU32>,
    config: &AppConfig,
    client_video: ClientVideoSupport,
    events: &Sender<HostEvent>,
    evrt_active: &Arc<Mutex<Option<SocketAddr>>>,
    client_max_res: &Arc<AtomicU64>,
    actual_encode_res: &Arc<AtomicU64>,
    want_evrtck: &Arc<AtomicBool>,
    cursor_pos: &Arc<AtomicU64>,
    evrtck_silicon_requested: &Arc<AtomicBool>,
    evrtck_return_requested: &Arc<AtomicBool>,
    evrtck_scheduler_silicon_active: &Arc<AtomicBool>,
) {
    // RustDesk-compatible service model: switching displays changes the
    // subscribed service set instead of mutating capture state inside a worker.
    let mut next: HashSet<i32> = displays.into_iter().map(|display| display.max(0)).collect();
    if next.is_empty() {
        next.insert(0);
    }

    let using_evrtck_now = want_evrtck.load(Ordering::Relaxed);
    let restart: Vec<i32> = next
        .intersection(subscribed)
        .copied()
        .filter(|display| {
            services
                .get(display)
                .map(|service| {
                    service.client_video != client_video || service.using_evrtck != using_evrtck_now
                })
                .unwrap_or(false)
        })
        .collect();
    let removed: Vec<i32> = subscribed
        .difference(&next)
        .copied()
        .chain(restart.iter().copied())
        .collect();
    for display in removed {
        if let Some(service) = services.remove(&display) {
            service.stop.store(true, Ordering::Relaxed);
            let _ = service.handle.join();
            log(
                events,
                format!("VideoService display={} stopped/restarting", display + 1),
            );
        }
    }

    let added: Vec<i32> = next
        .iter()
        .copied()
        .filter(|display| !services.contains_key(display))
        .collect();
    for display in added {
        // Android game mode: want_evrtck=false means the client requested H264/H265/AV1
        // (non-EVRTCK). Force StreamingMode::Game so static_skip is disabled — game
        // content changes every frame and static_skip causes the host to send ~12fps
        // instead of the configured 30fps.
        let mut eff_config = config.clone();
        if !want_evrtck.load(Ordering::Relaxed)
            && eff_config.display.streaming_mode == crate::settings::StreamingMode::Support
        {
            eff_config.display.streaming_mode = crate::settings::StreamingMode::Game;
        }
        let service = start_video_service(
            display,
            tcp_tx.clone(),
            evrt_tx.clone(),
            pipeline_stop.clone(),
            target_fps.clone(),
            quality_ms.clone(),
            bitrate_scale_milli.clone(),
            eff_config,
            client_video,
            events.clone(),
            evrt_active.clone(),
            client_max_res.clone(),
            actual_encode_res.clone(),
            want_evrtck.clone(),
            cursor_pos.clone(),
            evrtck_silicon_requested.clone(),
            evrtck_return_requested.clone(),
            evrtck_scheduler_silicon_active.clone(),
        );
        services.insert(display, service);
    }

    *subscribed = next;
    let mut ordered: Vec<i32> = subscribed.iter().copied().collect();
    ordered.sort_unstable();
    log(
        events,
        format!(
            "CaptureDisplays set={ordered:?} conn=(relay); services={}",
            services.len()
        ),
    );
    if let Some(display) = ordered.first().copied() {
        send_ordered_switch_display(tcp_tx, pipeline_stop, display);
    }
}

fn start_video_service(
    display: i32,
    tcp_tx: SyncSender<TcpItem>,
    evrt_tx: SyncSender<EncodedFrame>,
    pipeline_stop: Arc<AtomicBool>,
    target_fps: Arc<AtomicU32>,
    quality_ms: Arc<AtomicU32>,
    bitrate_scale_milli: Arc<AtomicU32>,
    config: AppConfig,
    client_video: ClientVideoSupport,
    events: Sender<HostEvent>,
    evrt_active: Arc<Mutex<Option<SocketAddr>>>,
    client_max_res: Arc<AtomicU64>,
    actual_encode_res: Arc<AtomicU64>,
    want_evrtck: Arc<AtomicBool>,
    cursor_pos: Arc<AtomicU64>,
    evrtck_silicon_requested: Arc<AtomicBool>,
    evrtck_return_requested: Arc<AtomicBool>,
    evrtck_scheduler_silicon_active: Arc<AtomicBool>,
) -> VideoServiceHandle {
    let service_stop = Arc::new(AtomicBool::new(false));
    let loop_stop = Arc::new(AtomicBool::new(false));
    let (idr_tx, idr_rx) = mpsc::channel::<()>();
    let service_stop_for_thread = service_stop.clone();
    let loop_stop_for_thread = loop_stop.clone();
    let events_for_log = events.clone();
    let using_evrtck_for_handle = want_evrtck.load(Ordering::Relaxed);

    let handle = thread::Builder::new()
        .name(format!("video-service-d{}", display + 1))
        .spawn(move || {
            log(
                &events_for_log,
                format!(
                    "VideoService display={} started fps={} codec={:?}",
                    display + 1,
                    target_fps.load(Ordering::Relaxed),
                    config.display.codec
                ),
            );

            let watcher_stop = loop_stop_for_thread.clone();
            let watcher = thread::Builder::new()
                .name(format!("video-service-stop-d{}", display + 1))
                .spawn(move || {
                    while !pipeline_stop.load(Ordering::Relaxed)
                        && !service_stop_for_thread.load(Ordering::Relaxed)
                        && !watcher_stop.load(Ordering::Relaxed)
                    {
                        thread::sleep(Duration::from_millis(25));
                    }
                    watcher_stop.store(true, Ordering::Relaxed);
                })
                .ok();

            encode_loop(
                display,
                loop_stop_for_thread,
                target_fps,
                quality_ms,
                bitrate_scale_milli,
                config,
                client_video,
                events_for_log,
                tcp_tx,
                evrt_tx,
                evrt_active,
                idr_rx,
                client_max_res,
                actual_encode_res,
                want_evrtck,
                cursor_pos,
                evrtck_silicon_requested,
                evrtck_return_requested,
                evrtck_scheduler_silicon_active,
            );

            if let Some(watcher) = watcher {
                let _ = watcher.join();
            }
        })
        .expect("spawn video service");

    VideoServiceHandle {
        stop: service_stop,
        idr_tx,
        handle,
        client_video,
        using_evrtck: using_evrtck_for_handle,
    }
}

fn send_ordered_switch_display(tcp_tx: &SyncSender<TcpItem>, stop: &Arc<AtomicBool>, display: i32) {
    let Some(msg) = switch_display_message(display) else {
        return;
    };

    while !stop.load(Ordering::Relaxed) {
        match tcp_tx.try_send(TcpItem::Peer(msg.clone())) {
            Ok(()) => break,
            Err(mpsc::TrySendError::Full(_)) => {
                thread::sleep(Duration::from_millis(5));
            }
            Err(mpsc::TrySendError::Disconnected(_)) => break,
        }
    }
}

fn evrtck_silicon_prefer(client: ClientVideoSupport, evrt_live: bool) -> Option<PreferCodec> {
    // Relay/TCP path is safest with H264. H265/AV1 over relay has platform
    // decoder edge cases in this codebase, so only prefer them once EVRT UDP is
    // live. H264 is also the broadest RustDesk-compatible fallback.
    if client.h264 {
        return Some(PreferCodec::H264);
    }
    if evrt_live && client.h265 {
        return Some(PreferCodec::H265);
    }
    if evrt_live && client.av1 {
        return Some(PreferCodec::Av1);
    }
    None
}

fn evrtck_return_candidate(
    roi: crate::evrt::RoiRect,
    width: u32,
    height: u32,
    want_idr: bool,
) -> bool {
    !want_idr && roi.dirty_area_milli(width, height) <= 80
}

fn switch_display_message(display: i32) -> Option<crate::rustdesk_proto::PeerMessage> {
    use crate::rustdesk_proto::{misc, peer_message, Misc, PeerMessage, SwitchDisplay};

    let displays = crate::capture::display_infos();
    let selected = displays
        .iter()
        .find(|info| info.index == display.max(0))
        .or_else(|| displays.first())?;

    Some(PeerMessage {
        union: Some(peer_message::Union::Misc(Misc {
            union: Some(misc::Union::SwitchDisplay(SwitchDisplay {
                display: selected.index,
                x: selected.x,
                y: selected.y,
                width: selected.width,
                height: selected.height,
                cursor_embedded: false,
            })),
        })),
    })
}

fn encode_loop(
    display: i32,
    stop: Arc<AtomicBool>,
    target_fps: Arc<AtomicU32>,
    quality_ms: Arc<AtomicU32>,
    bitrate_scale_milli: Arc<AtomicU32>,
    config: AppConfig,
    client_video: ClientVideoSupport,
    events: Sender<HostEvent>,
    tcp_tx: SyncSender<TcpItem>,
    evrt_tx: SyncSender<EncodedFrame>,
    evrt_active: Arc<Mutex<Option<SocketAddr>>>,
    idr_rx: Receiver<()>,
    client_max_res: Arc<AtomicU64>,
    actual_encode_res: Arc<AtomicU64>,
    want_evrtck: Arc<AtomicBool>,
    cursor_pos: Arc<AtomicU64>,
    evrtck_silicon_requested: Arc<AtomicBool>,
    evrtck_return_requested: Arc<AtomicBool>,
    evrtck_scheduler_silicon_active: Arc<AtomicBool>,
) {
    use crate::evrtck::{CopyRect, DirtyRect, EvrtckEncoder};
    use crate::host::{h264_target_bitrate_bps_pub, EncodedOutput, MultiEncoder};

    thread_local! {
        static EVRTCK_ENC: std::cell::RefCell<Option<EvrtckEncoder>> =
            std::cell::RefCell::new(None);
    }

    log(
        &events,
        format!("VideoService display={} encoder loop started", display + 1),
    );

    // ★ Единый каскад энкодеров: MF → VideoToolbox → NVENC → OpenH264 → PNG
    let mut encoder = MultiEncoder::new(config.display.encoder, config.display.codec, client_video);
    log(
        &events,
        format!("Encoder каскад: {}", encoder.backend_label()),
    );
    // Клиент запросил конкретный аппаратный кодек (игровой режим), а у нас нет
    // ни одного аппаратного энкодера, который он смог бы декодировать. Каскад
    // сейчас свалится в OpenH264/PNG — это не игровой режим. Пишем громко: без
    // этой строки диагностика выглядит как «игровой режим просто медленный».
    if !want_evrtck.load(Ordering::Relaxed) && !encoder.has_hardware_backend() {
        log(
            &events,
            format!(
                "⚠️ ИГРОВОЙ РЕЖИМ НЕВОЗМОЖЕН: клиент просит аппаратный кодек (prefer={:?}), \
                 но на этом хосте нет подходящего аппаратного энкодера \
                 (NVENC / Intel Quick Sync / AMD / VideoToolbox). \
                 Кодирование пойдёт софтом — клиенту будет медленнее, чем в обычном режиме.",
                client_video.prefer,
            ),
        );
    }
    let allow_static_skip = config.display.streaming_mode.allows_static_skip();
    log(
        &events,
        format!(
            "Streaming mode: {} (static_skip={})",
            config.display.streaming_mode.label(),
            allow_static_skip
        ),
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
    let mut evrtck_logged = false;
    let mut evrtck_silicon_candidate_logged_at: Option<Instant> = None;
    let mut evrtck_silicon_candidate_frames: u32 = 0;
    let mut evrtck_return_candidate_frames: u32 = 0;
    const TELE_INTERVAL: Duration = Duration::from_secs(10);
    const EVRTCK_SILICON_SWITCH_STREAK: u32 = 8;
    const EVRTCK_RETURN_STREAK: u32 = 90;

    // ★ Периодическая отправка телеметрии хоста КЛИЕНТУ (для --diagnose).
    //   Не один раз, а каждые 2с — надёжно доходит даже при дропах/статике.
    let mut last_host_tele_at = Instant::now();
    let mut host_tele_sent_base = 0_u64;
    let mut host_tele_skipped_base = 0_u64;
    let mut host_tele_samples_base = 0_u64;
    let mut host_tele_capture_base = 0_u64;
    let mut host_tele_change_base = 0_u64;
    let mut host_tele_encode_base = 0_u64;
    let mut host_tele_capture_thread_samples_base = 0_u64;
    let mut host_tele_capture_thread_us_base = 0_u64;
    const HOST_TELE_INTERVAL: Duration = Duration::from_secs(2);

    // ★ Адаптивный даунскейл для софт-энкодера.
    //   1440p/4K в OpenH264 = сотни мс/кадр. Если после первого кадра backend
    //   оказался софтверным — кодируем в пониженном разрешении (≤720p по высоте),
    //   клиент апскейлит в окне. Аппаратный энкодер — даунскейл не нужен.
    let mut downscale_to: Option<(u32, u32)> = None;
    let mut downscale_buf: Vec<u8> = Vec::new();
    let mut software_profile_active = false;
    // fps/quality as they were the moment the software profile clamped them —
    // restored verbatim if a hardware backend later takes over. Saved rather
    // than re-read because the software profile re-clamps `target_fps` every
    // iteration while active, so by then the client's real request is gone.
    let mut fps_before_software_profile: u32 = 0;
    let mut quality_before_software_profile: u32 = 0;

    // Capture double buffer
    type CapSlot = Arc<Mutex<Option<(i32, u32, u32, Vec<u8>, crate::capture::CaptureFrameMeta)>>>;
    let cap_slot: CapSlot = Arc::new(Mutex::new(None));
    let capture_thread_samples = Arc::new(AtomicU64::new(0));
    let capture_thread_us_total = Arc::new(AtomicU64::new(0));
    let capture_thread_us_max = Arc::new(AtomicU64::new(0));
    // Keep the handle so we can join the capture thread before NVENC is
    // destroyed.  Without this join, the DXGI duplication and NVENC D3D11
    // devices overlap during teardown → GPU TDR → system freeze.
    let cap_handle = {
        let cap_stop = stop.clone();
        let cap_fps = target_fps.clone();
        let cap_slot2 = cap_slot.clone();
        let cap_samples = capture_thread_samples.clone();
        let cap_us_total = capture_thread_us_total.clone();
        let cap_us_max = capture_thread_us_max.clone();
        thread::Builder::new()
            .name(format!("pipeline-capture-d{}", display + 1))
            .spawn(move || {
                let mut buf = Vec::new();
                let mut next_capture_due = Instant::now();
                let capture_spin = Duration::from_micros(500);
                loop {
                    if cap_stop.load(Ordering::Relaxed) {
                        break;
                    }
                    let fps = cap_fps.load(Ordering::Relaxed).clamp(5, 60);
                    let frame_interval = Duration::from_nanos(1_000_000_000 / fps as u64);
                    let now = Instant::now();
                    if now < next_capture_due {
                        let wait = next_capture_due - now;
                        if wait > capture_spin {
                            thread::sleep(wait - capture_spin);
                        } else {
                            std::hint::spin_loop();
                        }
                        continue;
                    }
                    next_capture_due += frame_interval;

                    let capture_started = Instant::now();
                    // Agentless VM-доступ: если клиент прикреплён к VM, кодируем
                    // кадр консоли VM вместо физического экрана хоста.
                    let captured = crate::vm_bridge::active_frame(&mut buf)
                        .map(|(w, h)| (w, h, crate::capture::CaptureFrameMeta::default()))
                        .or_else(|| {
                            crate::capture::capture_display_into_with_meta(display, &mut buf)
                        });
                    let capture_us = capture_started
                        .elapsed()
                        .as_micros()
                        .min(u128::from(u64::MAX)) as u64;
                    cap_samples.fetch_add(1, Ordering::Relaxed);
                    cap_us_total.fetch_add(capture_us, Ordering::Relaxed);
                    cap_us_max.fetch_max(capture_us, Ordering::Relaxed);

                    if let Some((w, h, meta)) = captured {
                        if let Ok(mut slot) = cap_slot2.lock() {
                            match slot.as_mut() {
                                Some(s) if s.0 == display && s.1 == w && s.2 == h => {
                                    std::mem::swap(&mut s.3, &mut buf);
                                    s.4 = meta;
                                }
                                _ => *slot = Some((display, w, h, buf.clone(), meta)),
                            }
                        }
                    }
                    if next_capture_due < Instant::now() {
                        next_capture_due = Instant::now() + frame_interval;
                    }
                }
                // Leak the DXGI D3D11 device before this thread exits.
                // Dropping it while WGPU/WGL is active races on the NVIDIA
                // KMD lock → deadlock → render thread freeze.  The OS reclaims
                // all GPU resources when the process exits normally.
                crate::capture::leak_capture_resources();
            })
            .expect("spawn capture")
    };

    let mut frame_id: u32 = 0;
    let mut last_idr: Instant = Instant::now();
    let mut force_recovery_key = true;
    // Взводится, когда EVRTCK-кадр закодирован (база энкодера уже сдвинулась),
    // но отправить его не удалось — клиент остался на старой базе. Снимается
    // только когда IDR реально ушёл. См. ветку `TrySendError::Full`.
    let mut evrtck_baseline_dirty = false;
    let mut next_frame_due: Instant = Instant::now();
    // Game-mode static-frame cache: DXGI AcquireNextFrame(0) on Windows 11 can
    // block until DWM composites (~8fps for static desktops) even with timeout=0.
    // To maintain 60fps we keep the last captured frame and reuse it when
    // cap_slot is empty. Static/identical P-frames compress to near-zero bitrate.
    let mut last_game_frame: Option<(i32, u32, u32, Vec<u8>, crate::capture::CaptureFrameMeta)> =
        None;
    // Safety counter for the evrt_tx Disconnected path: if evrt_active isn't
    // cleared within ~3 frames the loop would spin. Bail out after that.
    let mut evrt_disconnect_spins: u8 = 0;
    // H264/H265 over TCP relay: 1200ms — frequent IDR for packet-loss recovery.
    // H264/H265 over EVRT UDP: 4s — EVRT has FEC; no need for 1.2s storms.
    // EVRTCK: configurable (default 20s); IDR is a full frame ~850 KB.
    const IDR_MIN_H264: Duration = Duration::from_millis(1_200);
    // Reduced from 8s: codec-change IDR requests must not wait long (see evrt_session.rs).
    // 2s is still enough to prevent RefreshVideoDisplay storms over EVRT.
    const IDR_MIN_EVRT_HXXX: Duration = Duration::from_secs(2);
    let idr_interval_secs = config.display.idr_interval_secs.clamp(5, 120);
    let idr_min_evrtck = Duration::from_secs(u64::from(idr_interval_secs));
    const SPIN: Duration = Duration::from_micros(1_500);
    // Отслеживаем переход EVRTCK inactive→active: нужен IDR при первом подключении.
    let mut was_evrt_on = false;

    // VM-режим: отслеживаем frame_seq из vm_bridge.
    // Когда WMI прислал новый кадр (seq изменился) — обходим change_detector
    // и форсируем отправку. Это устраняет задержку 0–1200ms (IDR-таймер)
    // при вводе текста в терминал, где dirty_area < порога change_detector.
    let mut last_vm_seq: u64 = 0;

    while !stop.load(Ordering::Relaxed) {
        // Вычисляем evrt_on один раз в начале итерации — используется
        // везде ниже (fps-cap, want_idr, dispatch, bitrate-cap).
        let evrt_on = evrt_active.lock().map(|g| g.is_some()).unwrap_or(false);
        // using_evrtck: клиент поддерживает EVRTCK (want_evrtck=true) — НЕ зависит
        // от evrt_on. Раньше EVRTCK кодировался только когда прямой EVRT UDP уже
        // подключён; без него «обычный режим» тихо скатывался на H264/H265 через
        // TCP. Теперь EVRTCK кодируется всегда, когда клиент его хочет — при
        // evrt_on=true кадры едут по EVRT UDP как раньше, при evrt_on=false те же
        // EVRTCK-кадры едут по TCP relay (Misc::TcpEvrtckFrame, dispatch ниже).
        // want_evrtck фиксируется по первому SetClientCodec: Auto/VP9 → true, иначе false.
        // Это отличает игровой режим (H265 при старте → false) от обычного (Auto → true).
        // encode_loop не перезапускается при смене кодека (set_subscribed_displays no-op),
        // поэтому want_evrtck пробрасывается через Arc<AtomicBool> вместо локального prefer.
        let using_evrtck = want_evrtck.load(Ordering::Relaxed);
        let current_idr_min = if using_evrtck {
            idr_min_evrtck
        } else if evrt_on {
            IDR_MIN_EVRT_HXXX // H265/H264 через EVRT UDP — NACK/FEC покрывает потери
        } else {
            IDR_MIN_H264
        };

        // Любой переход кодека требует IDR на принимающей стороне:
        // • H264→EVRTCK: клиент должен получить полный первый кадр (не дельту).
        // • EVRTCK→H264: H264-референс был заморожен всё время пока шёл EVRTCK;
        //   первый P-frame будет дельтой против очень старого референса (потенциально
        //   МБ), поэтому принудительный IDR нужен и здесь.
        if evrt_on != was_evrt_on {
            force_recovery_key = true;
        }
        was_evrt_on = evrt_on;

        // Когда EVRTCK активен — не применяем software fps/quality caps:
        // мы не используем H264 encoder, и 30fps EVRTCK должен работать полную скорость.
        if software_profile_active && !using_evrtck {
            let current = target_fps.load(Ordering::Relaxed);
            let software_fps = software_encoder_target_fps(current);
            if current != software_fps {
                target_fps.store(software_fps, Ordering::Relaxed);
            }
            let current_quality = quality_ms.load(Ordering::Relaxed);
            let software_quality = software_encoder_quality_milli(current_quality);
            if current_quality != software_quality {
                quality_ms.store(software_quality, Ordering::Relaxed);
            }
        }
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

        // IDR по таймеру/запросу.
        //
        // Requested IDRs get their OWN, much shorter floor than the periodic
        // timer. They used to share `current_idr_min`, which for EVRTCK is the
        // configurable `idr_interval_secs` — 20 SECONDS by default. That gate
        // is right for the unrequested periodic refresh, but catastrophic for
        // the request path, because a request means the client has already
        // lost data and cannot decode anything until it gets an IDR:
        //
        //   client drops a frame (FEC couldn't recover it)
        //     → sends RequestKeyFrame and enters `wait_for_keyframe()`
        //     → `ChannelReassembler.waiting_after_loss` discards every P-frame
        //     → host's `evrt_session` also sets `waiting_for_idr` and stops
        //       forwarding non-IDR frames
        //     → and then BOTH sides sit there for up to 20 s
        //
        // i.e. one unrecovered packet froze the picture on the last good frame
        // for up to twenty seconds, which is exactly what a user reports as
        // "EVRTCK много артефактов" — not corrupted pixels, but stale content
        // that lingers and then snaps forward.
        //
        // The IDR-storm concern the old shared gate protected against is real
        // but is already handled one layer up: `evrt_session` rate-limits
        // EVRTCK keyframe requests to `IDR_RATELIMIT_EVRTCK` (500 ms). Use the
        // same 500 ms here so the two limits agree, instead of the outer one
        // silently overriding the inner by 40×.
        const IDR_MIN_REQUESTED: Duration = Duration::from_millis(500);
        let external_idr = idr_rx.try_recv().is_ok();
        while idr_rx.try_recv().is_ok() {} // drain burst — multiple requests count as one
                                           // `evrtck_baseline_dirty`: мы закодировали EVRTCK-кадр (сдвинув XOR-базу
                                           // энкодера), но не смогли его отправить — см. ветку TrySendError::Full
                                           // ниже. База клиента разошлась с нашей, лечится только IDR. Идёт по тому
                                           // же троттлируемому пути, что и запрос клиента.
        let recovery_idr = external_idr || evrtck_baseline_dirty;
        // Даже аварийный `force_recovery_key` подчиняется полу — кроме самого
        // первого кадра сессии, который обязан быть ключевым безусловно.
        //
        // Раньше он обходил все пороги, и на живой сессии это давало IDR на
        // СОСЕДНИХ кадрах (id=111,112 и 159,160,161 в логе хоста) по 50-128 КБ
        // каждый. При 60fps это десятки мегабит одних только ключевых кадров:
        // канал отправки захлёбывается, кадры дропаются, fps падает до
        // единиц, а вместе с видео встаёт и ввод — ровно та картина, которую
        // видел пользователь. Один пропущенный аварийный IDR стоит ≤300мс
        // ожидания; шторм стоит всей сессии.
        const IDR_MIN_FORCED: Duration = Duration::from_millis(300);
        let first_frame_ever = frame_id == 0;
        let forced_allowed =
            force_recovery_key && (first_frame_ever || last_idr.elapsed() >= IDR_MIN_FORCED);
        let periodic_key = forced_allowed
            || (recovery_idr && last_idr.elapsed() >= IDR_MIN_REQUESTED.min(current_idr_min))
            || last_idr.elapsed() > current_idr_min;

        // Захват
        let cap_started = Instant::now();
        let cap_result = cap_slot.lock().ok().and_then(|mut s| s.take());
        // Game mode: reuse cached last frame when DXGI has nothing new.
        // On Windows 11, AcquireNextFrame(0) blocks until DWM composites
        // (~8fps for static desktops), so cap_slot is only filled ~8×/s.
        // The cache lets us encode the same frame at 60fps; P-frames of
        // identical content cost near-zero bitrate via NVENC.
        let is_game_mode_enc = evrt_on && !want_evrtck.load(Ordering::Relaxed);
        let frame = if let Some(f) = cap_result {
            if is_game_mode_enc {
                last_game_frame = Some((f.0, f.1, f.2, f.3.clone(), f.4.clone()));
            }
            Some(f)
        } else if is_game_mode_enc {
            last_game_frame
                .as_ref()
                .map(|(d, w, h, data, meta)| (*d, *w, *h, data.clone(), meta.clone()))
        } else {
            None
        };
        let Some((display, cap_w, cap_h, bgra_raw, capture_meta)) = frame else {
            thread::sleep(Duration::from_millis(1));
            continue;
        };
        tele.mark_capture(cap_started.elapsed());
        let expected_bgra_len = (cap_w as usize)
            .saturating_mul(cap_h as usize)
            .saturating_mul(4);
        if cap_w == 0 || cap_h == 0 || bgra_raw.len() != expected_bgra_len {
            log(
                &events,
                format!(
                    "Capture returned invalid frame geometry: {}x{}, bgra={} expected={}",
                    cap_w,
                    cap_h,
                    bgra_raw.len(),
                    expected_bgra_len
                ),
            );
            force_recovery_key = true;
            thread::sleep(Duration::from_millis(10));
            continue;
        }

        // FSR: апскейл происходит in-place в буфере адаптера.
        // Передаём срез напрямую в кодировщик — без лишнего .to_owned()
        // (кодирование синхронно сразу после, буфер FSR жив весь кадр).
        let (enc_w, enc_h) = (cap_w, cap_h);
        let bgra: &[u8] = match fsr {
            Some(ref mut a) => a.process_bgra(&bgra_raw, cap_w, cap_h, enc_w, enc_h),
            None => &bgra_raw,
        };

        // ── VM-режим: bypass change_detector когда WMI прислал новый кадр ───
        // WMI thumbnail обновляется медленно (~3fps). Обычный change_detector
        // пропускает мелкие изменения текста (dirty_area < 8% порога) и не
        // отправляет кадр до следующего IDR (до 1200ms). В терминале без GUI
        // это делает ввод "невидимым" на 1-2 секунды после нажатия клавиши.
        // Решение: если seq изменился — WMI дал НОВЫЙ кадр → отправляем сразу.
        let vm_seq_changed = match crate::vm_bridge::vm_frame_seq() {
            Some(seq) => {
                if seq != last_vm_seq {
                    last_vm_seq = seq;
                    true
                } else {
                    false
                }
            }
            None => false,
        };

        // ── Детекция изменений: пропускаем статичные кадры ────────────────────
        let change_started = Instant::now();
        let decision = change_detector.decide(enc_w, enc_h, bgra, periodic_key || vm_seq_changed);
        tele.mark_change(change_started.elapsed());

        // В VM-режиме: если новый WMI-кадр — никогда не скипаем и не засыпаем.
        let skip_allowed = allow_static_skip && !vm_seq_changed;

        if !decision.send && skip_allowed {
            // Кадр не изменился — не кодируем, не шлём. Экономия трафика и CPU.
            tele.mark_skipped();
            // Backoff при долгой статике — снижаем частоту опроса.
            // В VM-режиме backoff НЕ применяется (vm_seq_changed отключил skip_allowed).
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

        // EVRTCK: не используем decision.force_key, который содержит idle_refresh
        // (каждые 2 сек при статичном экране → 1 МБ IDR зря). EVRTCK — lossless delta
        // codec, refresh не нужен. Изменения разрешения обрабатываются ниже через
        // recreation EvrtckEncoder (который ставит pending_keyframe сам).
        let mut want_idr = if evrt_on {
            periodic_key
        } else {
            decision.force_key || periodic_key
        };

        // Downscale для software encoder: не применяем когда EVRTCK активен,
        // т.к. мы не используем H264 и смена downscale_to не должна форсить IDR.
        if software_profile_active && !evrt_on {
            let next_downscale = software_encoder_downscale_target(enc_w, enc_h);
            if downscale_to != next_downscale {
                downscale_to = next_downscale;
                downscale_buf.clear();
                want_idr = true;
                force_recovery_key = true;
            }
        }

        // ★ Client resolution cap: если клиент сообщил max_width/max_height через
        //   ReceiverFeedback — даунскейлим до его экрана (меньше пикселей → NVENC
        //   кодирует быстрее, меньше бит, декодер клиента разгружен).
        //   Применяем только при EVRT (прямое UDP подключение к клиенту).
        if evrt_on {
            let packed = client_max_res.load(Ordering::Relaxed);
            if packed > 0 {
                let client_w = (packed >> 32) as u32;
                let client_h = (packed & 0xFFFF_FFFF) as u32;
                let next = client_cap_resolution(enc_w, enc_h, client_w, client_h);
                if downscale_to != next {
                    downscale_to = next;
                    downscale_buf.clear();
                    want_idr = true;
                }
            }
        }

        // ★ Применяем даунскейл если включён (софт-энкодер на высоком разрешении).
        let (enc_w, enc_h, bgra) = if let Some((dw, dh)) = downscale_to {
            downscale_bgra(bgra, enc_w, enc_h, &mut downscale_buf, dw, dh);
            (dw, dh, downscale_buf.as_slice())
        } else {
            (enc_w, enc_h, bgra)
        };
        // Публикуем фактическое разрешение кодирования для EVRT bitrate engine.
        let evrtck_copy_rects: Vec<CopyRect> = if enc_w == cap_w && enc_h == cap_h {
            capture_meta
                .move_rects
                .iter()
                .map(|rect| CopyRect {
                    src_x: rect.source_left,
                    src_y: rect.source_top,
                    dst_x: rect.dest.left,
                    dst_y: rect.dest.top,
                    width: rect.dest.width(),
                    height: rect.dest.height(),
                })
                .filter(|rect| rect.width > 0 && rect.height > 0)
                .collect()
        } else {
            Vec::new()
        };
        let evrtck_dirty_rects: Vec<DirtyRect> = if enc_w == cap_w && enc_h == cap_h {
            capture_meta
                .dirty_rects
                .iter()
                .map(|rect| DirtyRect {
                    left: rect.left,
                    top: rect.top,
                    right: rect.right,
                    bottom: rect.bottom,
                })
                .filter(|rect| rect.right > rect.left && rect.bottom > rect.top)
                .collect()
        } else {
            Vec::new()
        };
        actual_encode_res.store(((enc_w as u64) << 32) | (enc_h as u64), Ordering::Relaxed);

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
        if !evrt_on {
            const RELAY_MAX_BPS: u32 = 5_000_000; // безопасно для hbbr relay
            eff_bps = eff_bps.min(RELAY_MAX_BPS);
        }
        if software_profile_active {
            let cap_bps = software_encoder_bitrate_cap_bps(enc_w, enc_h, evrt_on);
            if software_text_quality_needs_floor(decision.roi, enc_w, enc_h, want_idr) {
                eff_bps =
                    eff_bps.max(software_encoder_bitrate_floor_bps(enc_w, enc_h).min(cap_bps));
            }
            eff_bps = eff_bps.min(cap_bps);
        }

        // ── Кодирование ────────────────────────────────────────────────────────
        // using_evrtck → EVRTCK lossless LAN кодек (всегда через EVRT UDP).
        // !using_evrtck → MultiEncoder (H264/H265/AV1): если evrt_on, кадры идут
        //   через EVRT UDP с выбранным кодеком; если нет — через TCP relay.
        let mut evrtck_analysis_for_frame: Option<crate::evrtck::FrameAnalysis> = None;
        let encode_started = Instant::now();
        let Some(out) = (if using_evrtck {
            Some(EVRTCK_ENC.with(|cell| {
                let mut enc = cell.borrow_mut();
                if enc
                    .as_ref()
                    .map(|e| e.width() != enc_w as usize || e.height() != enc_h as usize)
                    .unwrap_or(true)
                {
                    let mut fresh = EvrtckEncoder::new(enc_w as usize, enc_h as usize);
                    fresh.request_keyframe(); // сигнализируем клиенту сбросить prev-буфер
                    *enc = Some(fresh);
                }
                // IDR = сбросить prev в 0 → все тайлы dirty → клиент получает полный кадр.
                if want_idr {
                    enc.as_mut().unwrap().request_keyframe();
                }
                // EVRT2CKMAX-TASK-01 (experimental): последняя известная позиция курсора
                // клиента как Visible Region — dirty-тайлы рядом с курсором уходят на
                // проводе первыми. Координаты из MouseEvent — host screen space; если
                // включён downscale под client_max_res, точность страдает (курсор может
                // указывать чуть правее/ниже реального тайла), но это лабораторный путь:
                // set_focus_pixel клэмпит в границы кадра, деградация не хуже "фокус
                // немного смещён", не крэш и не порча данных.
                let packed_cursor = cursor_pos.load(Ordering::Relaxed);
                if packed_cursor != 0 {
                    let cx = (packed_cursor >> 32) as u32;
                    let cy = (packed_cursor & 0xFFFF_FFFF) as u32;
                    enc.as_mut().unwrap().set_focus_pixel(cx, cy);
                }
                let analysis = if evrtck_dirty_rects.is_empty() {
                    enc.as_ref().unwrap().analyze_next_frame(bgra)
                } else {
                    enc.as_ref()
                        .unwrap()
                        .analyze_next_frame_with_dirty_rects(bgra, &evrtck_dirty_rects)
                };
                evrtck_analysis_for_frame = Some(analysis);
                let (pkt, _stats) = if evrtck_copy_rects.is_empty() && evrtck_dirty_rects.is_empty()
                {
                    enc.as_mut()
                        .unwrap()
                        .encode_with_scroll_detection(bgra, frame_id)
                } else {
                    enc.as_mut()
                        .unwrap()
                        .encode_with_capture_hints(
                            bgra,
                            frame_id,
                            &evrtck_copy_rects,
                            &evrtck_dirty_rects,
                        )
                };
                if !evrtck_logged {
                    evrtck_logged = true;
                    log(
                        &events,
                        format!(
                            "EVRTCK encode: first frame id={frame_id} idr={want_idr} bytes={} dxgi_move_rects={} dxgi_dirty_rects={}",
                            pkt.data.len(),
                            evrtck_copy_rects.len(),
                            evrtck_dirty_rects.len()
                        ),
                    );
                }
                EncodedOutput {
                    bytes: pkt.data,
                    key: want_idr,
                    sps_pps: None,
                    codec: "evrtck",
                }
            }))
        } else {
            encoder.encode(enc_w, enc_h, fps, eff_bps, bgra, want_idr)
        }) else {
            continue;
        };
        let encode_dur = encode_started.elapsed();
        tele.mark_encode(encode_dur);
        let encode_ms = encode_dur.as_millis();
        if evrtck_scheduler_silicon_active.load(Ordering::Relaxed) && !using_evrtck {
            if evrtck_return_candidate(decision.roi, enc_w, enc_h, want_idr) {
                evrtck_return_candidate_frames = evrtck_return_candidate_frames.saturating_add(1);
                if evrtck_return_candidate_frames >= EVRTCK_RETURN_STREAK {
                    evrtck_return_requested.store(true, Ordering::Relaxed);
                }
            } else {
                evrtck_return_candidate_frames = 0;
            }
        } else {
            evrtck_return_candidate_frames = 0;
        }
        if let Some(analysis) = evrtck_analysis_for_frame {
            if analysis.prefer_silicon {
                evrtck_silicon_candidate_frames = evrtck_silicon_candidate_frames.saturating_add(1);
                if evrtck_silicon_candidate_frames >= EVRTCK_SILICON_SWITCH_STREAK {
                    evrtck_silicon_requested.store(true, Ordering::Relaxed);
                }
                let should_log = evrtck_silicon_candidate_logged_at
                    .map(|last| last.elapsed() >= Duration::from_secs(5))
                    .unwrap_or(true);
                if should_log {
                    evrtck_silicon_candidate_logged_at = Some(Instant::now());
                    log(
                        &events,
                        format!(
                            "EVRTCK scheduler: silicon candidate dirty={:.0}% entropy={:.2} est_payload={}B actual_payload={}B streak={}",
                            analysis.dirty_ratio * 100.0,
                            analysis.entropy_score,
                            analysis.estimated_payload_bytes,
                            out.bytes.len(),
                            evrtck_silicon_candidate_frames,
                        ),
                    );
                }
            } else {
                evrtck_silicon_candidate_frames = 0;
            }
        }

        // ★ Периодически шлём телеметрию хоста клиенту (надёжнее одноразовой).
        //   БЛОКИРУЮЩИЙ send — try_send дропался когда канал забит видео-кадрами.
        //   Телеметрия раз в 2с, блокировка на пару мс приемлема и гарантирует доставку.
        if last_host_tele_at.elapsed() >= HOST_TELE_INTERVAL {
            let host_tele_elapsed = last_host_tele_at.elapsed();
            last_host_tele_at = Instant::now();
            let capture_thread_samples_now = capture_thread_samples.load(Ordering::Relaxed);
            let capture_thread_us_now = capture_thread_us_total.load(Ordering::Relaxed);
            let capture_thread_max_us = capture_thread_us_max.swap(0, Ordering::Relaxed);

            if tele.sent_frames < host_tele_sent_base
                || tele.skipped_frames < host_tele_skipped_base
                || tele.samples < host_tele_samples_base
                || tele.capture_us_total < host_tele_capture_base
                || tele.change_us_total < host_tele_change_base
                || tele.encode_us_total < host_tele_encode_base
                || capture_thread_samples_now < host_tele_capture_thread_samples_base
                || capture_thread_us_now < host_tele_capture_thread_us_base
            {
                host_tele_sent_base = 0;
                host_tele_skipped_base = 0;
                host_tele_samples_base = 0;
                host_tele_capture_base = 0;
                host_tele_change_base = 0;
                host_tele_encode_base = 0;
                host_tele_capture_thread_samples_base = 0;
                host_tele_capture_thread_us_base = 0;
            }

            let sent_delta = tele.sent_frames.saturating_sub(host_tele_sent_base);
            let skipped_delta = tele.skipped_frames.saturating_sub(host_tele_skipped_base);
            let sample_delta = tele.samples.saturating_sub(host_tele_samples_base).max(1);
            let slot_us_delta = tele.capture_us_total.saturating_sub(host_tele_capture_base);
            let change_us_delta = tele.change_us_total.saturating_sub(host_tele_change_base);
            let encode_us_delta = tele.encode_us_total.saturating_sub(host_tele_encode_base);
            let capture_thread_samples_delta = capture_thread_samples_now
                .saturating_sub(host_tele_capture_thread_samples_base)
                .max(1);
            let capture_thread_us_delta =
                capture_thread_us_now.saturating_sub(host_tele_capture_thread_us_base);
            let actual_fps = sent_delta as f64 / host_tele_elapsed.as_secs_f64().max(0.001);
            let capture_avg_ms = capture_thread_us_delta / capture_thread_samples_delta / 1000;
            let slot_avg_ms = slot_us_delta / sample_delta / 1000;
            let change_avg_ms = change_us_delta / sample_delta / 1000;
            let encode_avg_ms = if sent_delta > 0 {
                encode_us_delta / sent_delta / 1000
            } else {
                0
            };
            let evrtck_analysis = evrtck_analysis_for_frame;
            let evrtck_dirty_pct = evrtck_analysis
                .map(|a| (a.dirty_ratio * 100.0).round() as u32)
                .unwrap_or(0);
            let evrtck_entropy_pct = evrtck_analysis
                .map(|a| (a.entropy_score * 100.0).round() as u32)
                .unwrap_or(0);
            let evrtck_est_payload = evrtck_analysis
                .map(|a| a.estimated_payload_bytes)
                .unwrap_or(0);
            let evrtck_silicon_candidate =
                evrtck_analysis.map(|a| a.prefer_silicon).unwrap_or(false);

            let info = format!(
                "backend={} encode_ms={} encode_avg_ms={} capture_avg_ms={} capture_max_ms={} slot_avg_ms={} change_avg_ms={} actual_fps={:.1} sent={} skipped={} interval_ms={} res={}x{} fps={} evrtck_dirty_pct={} evrtck_entropy_pct={} evrtck_est_payload={} evrtck_silicon_candidate={} build={}",
                encoder.active_backend(),
                encode_ms,
                encode_avg_ms,
                capture_avg_ms,
                capture_thread_max_us / 1000,
                slot_avg_ms,
                change_avg_ms,
                actual_fps,
                sent_delta,
                skipped_delta,
                host_tele_elapsed.as_millis(),
                enc_w, enc_h, fps,
                evrtck_dirty_pct,
                evrtck_entropy_pct,
                evrtck_est_payload,
                evrtck_silicon_candidate,
                crate::host::binary_build_stamp(),
            );
            // Шлём клиенту
            let tele_msg = crate::rustdesk_proto::PeerMessage {
                union: Some(crate::rustdesk_proto::peer_message::Union::Misc(
                    crate::rustdesk_proto::Misc {
                        union: Some(crate::rustdesk_proto::misc::Union::HostTelemetry(
                            info.clone(),
                        )),
                    },
                )),
            };
            // Never block here during shutdown: the TCP sender may be draining
            // its write timeout (up to 2s) which, combined with other overheads,
            // can push past the 3-second pipeline-join deadline in host.rs.
            // During normal operation stop=false, so we use blocking send to
            // guarantee delivery. Once stop is set we're about to exit anyway.
            if stop.load(Ordering::Relaxed) {
                let _ = tcp_tx.try_send(TcpItem::Peer(tele_msg));
            } else {
                let _ = tcp_tx.send(TcpItem::Peer(tele_msg));
            }
            // ★ ПИШЕМ свою диагностику в файл — видно хост напрямую, без клиента.
            write_host_diag(
                &info,
                skipped_delta,
                sent_delta,
                host_tele_elapsed.as_millis(),
                actual_fps,
            );

            host_tele_sent_base = tele.sent_total();
            host_tele_skipped_base = tele.skipped_total();
            host_tele_samples_base = tele.samples;
            host_tele_capture_base = tele.capture_us_total;
            host_tele_change_base = tele.change_us_total;
            host_tele_encode_base = tele.encode_us_total;
            host_tele_capture_thread_samples_base = capture_thread_samples_now;
            host_tele_capture_thread_us_base = capture_thread_us_now;
        }

        // ★ Один раз логируем РЕАЛЬНЫЙ бэкенд (MediaFoundation/OpenH264-SW/PNG).
        //   Критично для диагностики: показывает, аппаратный энкодер или софт.
        if !backend_logged {
            backend_logged = true;
            log(
                &events,
                format!(
                    "★ Реальный энкодер: {} ({}×{}@{}, первый кадр {}мс)",
                    // В EVRTCK-режиме MultiEncoder не используется вообще
                    // (тайлы кодирует EvrtckEncoder), поэтому active_backend
                    // остаётся "none" — в логе это читалось как «энкодера нет»
                    // и уводило диагностику в сторону. Пишем честно.
                    if using_evrtck {
                        "EVRTCK (CPU-тайлы)"
                    } else {
                        encoder.active_backend()
                    },
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

        // ★ Софт-энкодер на высоком разрешении → даунскейл + пониженный fps.
        //   Цель: высота ≤ 720 (сохраняя пропорции). Аппаратный — не трогаем.
        //
        //   Пересчитывается на КАЖДОЙ смене бэкенда, а не один раз на первом
        //   кадре. Раньше весь этот профиль решался внутри `if !backend_logged`
        //   выше — и если первый кадр случайно выдал OpenH264-SW (обычное дело
        //   на холодном старте: NVENC/MF инициализируются лениво, первый кадр
        //   успевает уйти через софт), сессия НАВСЕГДА оставалась на 15fps,
        //   даунскейле и низком битрейте. Живо наблюдалось на Intel-ПК
        //   пользователя: «★ Реальный энкодер: OpenH264-SW … @15fps», а уже
        //   следующей строкой телеметрии `backend=MediaFoundation encode_ms=8`
        //   — железо подхватило кодирование, но actual_fps так и остался 15.0
        //   до конца сессии.
        //
        //   Решение принимается по НАЛИЧИЮ аппаратного бэкенда, а не по тому,
        //   кто выдал последний конкретный кадр. Это принципиально: каскад
        //   `MultiEncoder::encode` пробует MF, и если тот вернул `Ok(None)`
        //   (аппаратный MFT асинхронный, ему случается нужен ещё вход, прежде
        //   чем отдать пакет), управление проваливается в OpenH264 — и
        //   `active_backend` на этом кадре честно становится софтверным, хотя
        //   железо никуда не делось. Первая версия этой проверки смотрела
        //   именно на `active_backend`, и на живом Intel-хосте профиль начал
        //   ДРЕБЕЗЖАТЬ: в логе «Software encoder profile» и «Аппаратный
        //   энкодер подхватил» чередовались каждую секунду, каждый раз
        //   переставляя fps/разрешение и форся IDR — fps просел до 3-6.
        //   `has_active_hardware_backend()` меняется только когда бэкенд
        //   действительно выключился из-за ошибки, то есть по-настоящему редко.
        let backend = encoder.active_backend();
        let is_software = !encoder.has_active_hardware_backend();
        if is_software != software_profile_active {
            software_profile_active = is_software;
            if is_software {
                fps_before_software_profile = target_fps.load(Ordering::Relaxed);
                quality_before_software_profile = quality_ms.load(Ordering::Relaxed);
                let software_fps = software_encoder_target_fps(target_fps.load(Ordering::Relaxed));
                if software_fps < fps {
                    target_fps.store(software_fps, Ordering::Relaxed);
                    next_frame_due = Instant::now();
                }
                let software_quality =
                    software_encoder_quality_milli(quality_ms.load(Ordering::Relaxed));
                quality_ms.store(software_quality, Ordering::Relaxed);

                downscale_to = software_encoder_downscale_target(enc_w, enc_h);
                let (dw, dh) = downscale_to.unwrap_or((enc_w, enc_h));
                let cap_bps = software_encoder_bitrate_cap_bps(dw, dh, evrt_on);
                let floor_bps = software_encoder_bitrate_floor_bps(dw, dh).min(cap_bps);

                log(
                    &events,
                    format!(
                        "Software encoder profile: {}x{} -> {}x{} @ {}fps, quality={}, bitrate {}-{} kbps, priority=text",
                        enc_w,
                        enc_h,
                        dw,
                        dh,
                        software_fps,
                        software_quality,
                        floor_bps / 1_000,
                        cap_bps / 1_000,
                    ),
                );
            } else {
                // Железо подхватило кодирование — снимаем софтверные ограничения
                // и возвращаем то, что клиент реально просил. Разрешение меняется,
                // поэтому нужен IDR: у клиента иначе останется декодер, настроенный
                // на старый (уменьшенный) размер кадра.
                // Не чистим downscale_buf: `downscale_bgra` начинает с
                // `dst.resize(...)` и перезаписывает буфер целиком, так что
                // старое содержимое никогда не может «просочиться». (Заодно
                // это единственный вариант, который здесь вообще компилируется
                // — на этой строке буфер ещё одолжен как `bgra` для encode.)
                if downscale_to.is_some() {
                    downscale_to = None;
                    force_recovery_key = true;
                }
                let restored_fps = fps_before_software_profile.clamp(5, 60);
                if restored_fps > 0 && target_fps.load(Ordering::Relaxed) < restored_fps {
                    target_fps.store(restored_fps, Ordering::Relaxed);
                    next_frame_due = Instant::now();
                }
                if quality_before_software_profile > 0
                    && quality_ms.load(Ordering::Relaxed) < quality_before_software_profile
                {
                    quality_ms.store(quality_before_software_profile, Ordering::Relaxed);
                }
                log(
                    &events,
                    format!(
                        "★ Аппаратный энкодер подхватил ({backend}) — софт-профиль снят: {restored_fps}fps, полное разрешение {enc_w}×{enc_h}",
                    ),
                );
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
            // EVRT primary: все кадры → EVRT UDP. НЕ шлём видео по TCP — клиент
            // всё равно не может декодировать H264 (OpenH264 Native:4 ошибки), а
            // эти ошибки триггерят RefreshVideoDisplay → IDR-шторм ~1 MB/запрос.
            // TCP relay остаётся только для control-сообщений (telemetry, shell).
            match evrt_tx.try_send(frame.clone()) {
                Ok(()) => {
                    evrt_disconnect_spins = 0;
                    if is_idr {
                        force_recovery_key = false;
                        // IDR реально ушёл — базы снова сошлись.
                        evrtck_baseline_dirty = false;
                    }
                }
                Err(mpsc::TrySendError::Full(_)) => {
                    // Канал занят: evrt_session ещё отправляет крупный IDR (100 Мбит/с ≈ 80мс/МБ).
                    // Дропаем этот кадр — форсить новый IDR безусловно нельзя,
                    // это как раз и даёт каскад Full → IDR → Full → IDR.
                    //
                    // НО для EVRTCK «просто дропнуть» недостаточно, и старое
                    // утверждение «клиент видит последний декодированный кадр»
                    // здесь неверно. Оно справедливо для H264/H265, где ссылки
                    // ведёт сам декодер. EVRTCK же — XOR-дельта против кадра,
                    // который энкодер УЖЕ сдвинул в момент кодирования: раз кадр
                    // закодирован, база хоста стала N, а у клиента осталась N-1.
                    // Дальше каждая следующая дельта ложится на чужую базу —
                    // картинка рассыпается (призрачные смещённые копии окон,
                    // блоки шума) и сама не чинится до следующего IDR. Живо
                    // воспроизводится перетаскиванием окна: кадры большие,
                    // канал (ёмкость 2) забивается, дропы идут пачками.
                    //
                    // Поэтому помечаем базу испорченной и просим IDR через
                    // ТРОТТЛИРУЕМЫЙ путь (IDR_MIN_REQUESTED, 500мс), а не через
                    // `force_recovery_key`, который обходит все ограничения и
                    // ровно этот каскад бы и устроил.
                    if using_evrtck {
                        evrtck_baseline_dirty = true;
                    }
                }
                Err(mpsc::TrySendError::Disconnected(_)) => {
                    // evrt_send_loop clears evrt_active=None when the session ends,
                    // so the next iteration normally sees evrt_on=false and
                    // falls back to H264. Force IDR so the client gets a clean frame.
                    force_recovery_key = true;
                    evrt_disconnect_spins += 1;
                    if evrt_disconnect_spins >= 3 {
                        // evrt_active wasn't cleared — evrt_send_loop may have panicked.
                        // Break to avoid spinning; the process watchdog will restart.
                        break;
                    }
                    continue;
                }
            }
        } else {
            // TCP relay is bounded and latency-sensitive. Drop video when the
            // sender is backed up so control messages and shutdown can still
            // make progress; the next captured frame will be fresher anyway.
            match send_tcp_video_frame(&tcp_tx, &stop, frame) {
                TcpVideoSend::Sent => {
                    if is_idr {
                        force_recovery_key = false;
                    }
                }
                TcpVideoSend::Dropped => {
                    force_recovery_key = true;
                    apply_tcp_backpressure(&bitrate_scale_milli);
                }
                TcpVideoSend::Disconnected => break,
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
    let _ = cap_handle.join();

    log(
        &events,
        format!("VideoService display={} encoder loop stopped", display + 1),
    );
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
        misc, peer_message, video_frame, EncodedVideoFrame, EncodedVideoFrames, Misc, PeerMessage,
        VideoFrame,
    };
    debug_assert!(
        frame.width > 0 && frame.height > 0,
        "encoded frame has invalid geometry: {}x{}",
        frame.width,
        frame.height
    );
    // EVRTCK — self-describing wire bytes (magic+version+dims baked in), едет
    // как Misc::TcpEvrtckFrame, а не VideoFrame: RustDesk-совместимый VideoFrame
    // union жёстко ограничен H264/H265/VP8/VP9/AV1 — заворачивать в него тайлы
    // EVRTCK означало бы, что клиент попытается скормить их H264-декодеру
    // (именно так раньше «обычный режим» без прямого EVRT молча скатывался на
    // H264/H265, что и привело к краху системного HEVC-декодера на части машин).
    if frame.codec == "evrtck" {
        return PeerMessage {
            union: Some(peer_message::Union::Misc(Misc {
                union: Some(misc::Union::TcpEvrtckFrame((*frame.bytes).clone())),
            })),
        };
    }
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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum TcpVideoSend {
    Sent,
    Dropped,
    Disconnected,
}

fn send_tcp_video_frame(
    tcp_tx: &SyncSender<TcpItem>,
    stop: &Arc<AtomicBool>,
    frame: EncodedFrame,
) -> TcpVideoSend {
    if !frame.is_idr {
        return match tcp_tx.try_send(TcpItem::Video(frame)) {
            Ok(()) => TcpVideoSend::Sent,
            Err(mpsc::TrySendError::Full(_)) => TcpVideoSend::Dropped,
            Err(mpsc::TrySendError::Disconnected(_)) => TcpVideoSend::Disconnected,
        };
    }

    let deadline = Instant::now() + Duration::from_millis(120);
    let mut item = TcpItem::Video(frame);
    loop {
        match tcp_tx.try_send(item) {
            Ok(()) => return TcpVideoSend::Sent,
            Err(mpsc::TrySendError::Full(returned)) => {
                if stop.load(Ordering::Relaxed) || Instant::now() >= deadline {
                    return TcpVideoSend::Dropped;
                }
                item = returned;
                thread::sleep(Duration::from_millis(2));
            }
            Err(mpsc::TrySendError::Disconnected(_)) => return TcpVideoSend::Disconnected,
        }
    }
}

fn apply_tcp_backpressure(bitrate_scale_milli: &AtomicU32) {
    let current = bitrate_scale_milli.load(Ordering::Relaxed);
    let reduced = (current.saturating_mul(85) / 100).max(MIN_BITRATE_SCALE_MILLI);
    if reduced < current {
        bitrate_scale_milli.store(reduced, Ordering::Relaxed);
    }
}

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
    idr_request_tx: Sender<()>,
    peer_id: String,
    evrt_token: Option<String>,
    client_max_res: Arc<AtomicU64>,
    actual_encode_res: Arc<AtomicU64>,
    want_evrtck: Arc<AtomicBool>,
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
        if let Ok((len, src)) = socket.recv_from(&mut buf) {
            if evrt_token.is_some() {
                let token_ok = crate::evrt::parse_authenticated(&buf, len, evrt_token.as_deref())
                    .is_some_and(|pkt| {
                        pkt.packet_type == crate::evrt::TYPE_CONTROL
                            && crate::evrt::control_token_matches(
                                &pkt.payload,
                                evrt_token.as_deref(),
                            )
                            && crate::evrt::parse_control(&pkt.payload).is_some()
                    });
                if !token_ok {
                    continue;
                }
            }
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
        session_token: evrt_token,
        socket: socket.clone(),
        config: config.clone(),
        peer_id: peer_id.clone(),
        events: events.clone(),
        stop: stop.clone(),
        frame_rx, // ← готовые кадры из encoder, не своя запись
        target_fps,
        quality_milli: quality_ms,
        bitrate_scale_milli,
        idr_request_tx,
        client_max_res,
        actual_encode_res,
        want_evrtck,
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
    fn skipped_total(&self) -> u64 {
        self.skipped_frames
    }
    fn sent_total(&self) -> u64 {
        self.sent_frames
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

// Software fallback policy: keep 1080p text native, reduce FPS first, and
// downscale only frames that are larger than a 1080p-class desktop.
const SOFTWARE_ENCODER_MAX_FPS: u32 = 15;
const SOFTWARE_ENCODER_MIN_QUALITY_MILLI: u32 = 1_000;
const SOFTWARE_ENCODER_MAX_QUALITY_MILLI: u32 = 1_800;
const SOFTWARE_NATIVE_MAX_W: u32 = 1920;
const SOFTWARE_NATIVE_MAX_H: u32 = 1080;
const SOFTWARE_NATIVE_MAX_PIXELS: u64 = 1920 * 1080;

fn software_encoder_target_fps(fps: u32) -> u32 {
    fps.clamp(5, SOFTWARE_ENCODER_MAX_FPS)
}

fn software_encoder_quality_milli(quality_milli: u32) -> u32 {
    quality_milli.clamp(
        SOFTWARE_ENCODER_MIN_QUALITY_MILLI,
        SOFTWARE_ENCODER_MAX_QUALITY_MILLI,
    )
}

/// Возвращает целевое разрешение для даунскейла под экран клиента.
/// Если хостовое разрешение уже помещается — None (даунскейл не нужен).
fn client_cap_resolution(
    src_w: u32,
    src_h: u32,
    client_w: u32,
    client_h: u32,
) -> Option<(u32, u32)> {
    if src_w == 0 || src_h == 0 || client_w == 0 || client_h == 0 {
        return None;
    }
    if src_w <= client_w && src_h <= client_h {
        return None; // уже вмещается, даунскейл не нужен
    }
    let (dw, dh) = scale_even_to_fit(src_w, src_h, client_w, client_h);
    (dw != src_w || dh != src_h).then_some((dw, dh))
}

fn software_encoder_downscale_target(width: u32, height: u32) -> Option<(u32, u32)> {
    if width == 0 || height == 0 {
        return None;
    }

    let pixels = u64::from(width) * u64::from(height);
    if width <= SOFTWARE_NATIVE_MAX_W
        && height <= SOFTWARE_NATIVE_MAX_H
        && pixels <= SOFTWARE_NATIVE_MAX_PIXELS
    {
        return None;
    }

    let (dw, dh) = scale_even_to_fit(width, height, SOFTWARE_NATIVE_MAX_W, SOFTWARE_NATIVE_MAX_H);
    (dw != width || dh != height).then_some((dw, dh))
}

fn scale_even_to_fit(width: u32, height: u32, max_w: u32, max_h: u32) -> (u32, u32) {
    if width == 0 || height == 0 || max_w == 0 || max_h == 0 {
        return (width, height);
    }

    let scale_w = max_w as f32 / width as f32;
    let scale_h = max_h as f32 / height as f32;
    let scale = scale_w.min(scale_h).min(1.0);
    let dw = ((width as f32 * scale) as u32 & !1).max(2);
    let dh = ((height as f32 * scale) as u32 & !1).max(2);
    (dw, dh)
}

fn software_encoder_bitrate_cap_bps(width: u32, height: u32, evrt_on: bool) -> u32 {
    let pixels = u64::from(width) * u64::from(height);
    let relay_cap = if pixels >= 1_900_000 {
        4_500_000
    } else if pixels >= 800_000 {
        3_000_000
    } else {
        2_000_000
    };

    if evrt_on {
        (relay_cap + 2_000_000).min(8_000_000)
    } else {
        relay_cap
    }
}

fn software_encoder_bitrate_floor_bps(width: u32, height: u32) -> u32 {
    let pixels = u64::from(width) * u64::from(height);
    if pixels >= 1_900_000 {
        3_200_000
    } else if pixels >= 800_000 {
        2_000_000
    } else {
        1_000_000
    }
}

fn software_text_quality_needs_floor(
    roi: crate::evrt::RoiRect,
    width: u32,
    height: u32,
    want_idr: bool,
) -> bool {
    want_idr || roi.dirty_area_milli(width, height) >= 80
}

/// Даунскейл BGRA-кадра в `dst` методом усреднения блоков (box filter).
/// Быстрый, без зависимостей. Для софт-энкодера на высоком разрешении.
fn downscale_bgra(src: &[u8], src_w: u32, src_h: u32, dst: &mut Vec<u8>, dst_w: u32, dst_h: u32) {
    let (sw, sh, dw, dh) = (
        src_w as usize,
        src_h as usize,
        dst_w as usize,
        dst_h as usize,
    );
    dst.resize(dw * dh * 4, 0);
    if sw == 0 || sh == 0 || dw == 0 || dh == 0 || src.len() < sw * sh * 4 {
        return;
    }

    // Целочисленные шаги (fixed-point 16.16) для семплирования центра блока.
    let x_ratio = ((sw << 16) / dw) as u64;
    let y_ratio = ((sh << 16) / dh) as u64;

    for dy in 0..dh {
        let sy = ((dy as u64 * y_ratio) >> 16) as usize;
        let src_row = sy * sw * 4;
        let dst_row = dy * dw * 4;
        for dx in 0..dw {
            let sx = ((dx as u64 * x_ratio) >> 16) as usize;
            let s = src_row + sx * 4;
            let d = dst_row + dx * 4;
            if s + 3 < src.len() && d + 3 < dst.len() {
                dst[d] = src[s];
                dst[d + 1] = src[s + 1];
                dst[d + 2] = src[s + 2];
                dst[d + 3] = 255;
            }
        }
    }
}

fn log(events: &Sender<HostEvent>, msg: String) {
    eprintln!("[pipeline] {msg}");
    let _ = events.send(HostEvent::Log(msg));
}

/// ★ Хост пишет свою диагностику в файл — видно энкодер/fps/build напрямую,
/// без клиента и без догадок. Перезаписывается каждые 2с активной сессии.
fn write_host_diag(info: &str, skipped: u64, sent: u64, interval_ms: u128, actual_fps: f64) {
    use std::time::{SystemTime, UNIX_EPOCH};
    let ts = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let body = format!(
        "# Хост-диагностика EvertyDesk Lite (живая)\n\n\
         Обновлено: unix {ts}\n\n\
         ## Энкодер\n\
         {info}\n\n\
         ## Кадры за последний интервал\n\
         - interval_ms: {interval_ms}\n\
         - actual_fps: {actual_fps:.1}\n\
         - sent: {sent}\n\
         - skipped (статика): {skipped}\n\n\
         > Это пишет САМ ХОСТ из encode_loop. Если файл свежий (unix растёт) и\n\
         > backend/encode_ms заполнены — хост точно на свежем билде.\n\
         > `backend=OpenH264-SW encode_ms>100` = софт (нет NVENC/MF аппаратного).\n\
         > `backend=NVENC encode_ms<10` = аппаратный RTX.\n",
    );
    let _ = std::fs::create_dir_all("diagnostics");
    let _ = std::fs::write("diagnostics/host_diag.md", body);
}

#[cfg(test)]
mod downscale_tests {
    use super::*;

    #[test]
    fn downscale_halves_dimensions() {
        // 4x4 → 2x2, заполнено байтом 100
        let src = vec![100u8; 4 * 4 * 4];
        let mut dst = Vec::new();
        downscale_bgra(&src, 4, 4, &mut dst, 2, 2);
        assert_eq!(dst.len(), 2 * 2 * 4);
        // Все пиксели должны быть ~100 (B/G/R), альфа 255
        for px in dst.chunks(4) {
            assert_eq!(px[0], 100);
            assert_eq!(px[3], 255);
        }
    }

    #[test]
    fn downscale_handles_empty() {
        let mut dst = Vec::new();
        downscale_bgra(&[], 0, 0, &mut dst, 2, 2);
        assert_eq!(dst.len(), 2 * 2 * 4); // resized но нули
    }
    #[test]
    fn software_profile_keeps_full_hd_native_for_text() {
        assert_eq!(software_encoder_downscale_target(1920, 1080), None);
    }

    #[test]
    fn software_profile_scales_4k_to_full_hd_class() {
        assert_eq!(
            software_encoder_downscale_target(3840, 2160),
            Some((1920, 1080))
        );
    }

    #[test]
    fn software_profile_preserves_ultrawide_aspect_ratio() {
        assert_eq!(
            software_encoder_downscale_target(2560, 1080),
            Some((1920, 810))
        );
    }

    #[test]
    fn software_profile_has_readable_full_hd_bitrate_floor() {
        assert!(software_encoder_bitrate_floor_bps(1920, 1080) >= 3_000_000);
        assert!(software_encoder_bitrate_cap_bps(1920, 1080, false) >= 4_000_000);
    }
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
    fn evrtck_silicon_prefer_uses_h264_on_relay_when_available() {
        let client = ClientVideoSupport {
            h264: true,
            h265: true,
            av1: true,
            prefer: PreferCodec::Auto,
        };

        assert_eq!(
            evrtck_silicon_prefer(client, false),
            Some(PreferCodec::H264)
        );
    }

    #[test]
    fn evrtck_silicon_prefer_allows_h265_when_evrt_is_live_and_h264_missing() {
        let client = ClientVideoSupport {
            h264: false,
            h265: true,
            av1: false,
            prefer: PreferCodec::Auto,
        };

        assert_eq!(evrtck_silicon_prefer(client, true), Some(PreferCodec::H265));
    }

    #[test]
    fn evrtck_silicon_prefer_does_not_switch_without_compatible_codec() {
        let client = ClientVideoSupport {
            h264: false,
            h265: true,
            av1: false,
            prefer: PreferCodec::Auto,
        };

        assert_eq!(evrtck_silicon_prefer(client, false), None);
    }

    #[test]
    fn evrtck_return_candidate_accepts_small_non_key_delta() {
        let roi = crate::evrt::RoiRect {
            frame_id: 1,
            x: 0,
            y: 0,
            w: 160,
            h: 90,
        };

        assert!(evrtck_return_candidate(roi, 1920, 1080, false));
    }

    #[test]
    fn evrtck_return_candidate_rejects_fullscreen_or_keyframe() {
        let fullscreen = crate::evrt::RoiRect {
            frame_id: 1,
            x: 0,
            y: 0,
            w: 0,
            h: 0,
        };
        let small = crate::evrt::RoiRect {
            frame_id: 1,
            x: 0,
            y: 0,
            w: 160,
            h: 90,
        };

        assert!(!evrtck_return_candidate(fullscreen, 1920, 1080, false));
        assert!(!evrtck_return_candidate(small, 1920, 1080, true));
    }

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
    fn tcp_backpressure_reduces_bitrate_scale() {
        let scale = AtomicU32::new(1_000);
        apply_tcp_backpressure(&scale);
        assert_eq!(scale.load(Ordering::Relaxed), 850);

        for _ in 0..20 {
            apply_tcp_backpressure(&scale);
        }
        assert_eq!(scale.load(Ordering::Relaxed), MIN_BITRATE_SCALE_MILLI);
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

    #[test]
    fn evrtck_frame_routes_to_misc_not_h264_wrapper() {
        // Regression for "normal mode always EVRTCK": before this fix, any
        // frame reaching make_tcp_video_frame() with evrt_on=false got wrapped
        // as VideoFrame::H264s regardless of actual codec — for codec=="evrtck"
        // that means the client's H264 decoder would receive raw EVRTCK tile
        // bytes as if they were H264 NAL units.
        use crate::evrtck::{EvrtckDecoder, EvrtckEncoder};
        use crate::rustdesk_proto::{misc, peer_message};

        let (w, h) = (64usize, 64usize);
        let mut bgra = vec![0u8; w * h * 4];
        for (i, px) in bgra.chunks_exact_mut(4).enumerate() {
            px[0] = (i % 251) as u8;
            px[2] = ((i / 3) % 251) as u8;
            px[3] = 255;
        }
        let mut enc = EvrtckEncoder::new(w, h);
        let pkt = enc.encode(&bgra, 1);

        let frame = EncodedFrame {
            bytes: std::sync::Arc::new(pkt.data),
            is_idr: true,
            frame_id: 1,
            pts_us: 0,
            display: 0,
            sps_pps: None,
            width: w as u32,
            height: h as u32,
            codec: "evrtck",
            roi: crate::evrt::RoiRect {
                frame_id: 1,
                x: 0,
                y: 0,
                w: 0,
                h: 0,
            },
        };

        let msg = make_tcp_video_frame(&frame);

        // Must be Misc::TcpEvrtckFrame, NOT VideoFrame::H264s.
        let wire_bytes = match msg.union {
            Some(peer_message::Union::Misc(m)) => match m.union {
                Some(misc::Union::TcpEvrtckFrame(bytes)) => bytes,
                other => panic!("expected TcpEvrtckFrame, got {other:?}"),
            },
            other => {
                panic!("expected Misc, got {other:?} — EVRTCK frame was wrapped as VideoFrame")
            }
        };

        // And the client's EVRTCK decoder must be able to make sense of it.
        let mut dec = EvrtckDecoder::new();
        let rgba_len = dec
            .decode_wire(&wire_bytes)
            .expect("decode_wire must succeed")
            .len();
        assert_eq!(dec.width(), w);
        assert_eq!(dec.height(), h);
        assert_eq!(rgba_len, w * h * 4);
    }

    #[test]
    fn h264_frame_still_routes_to_video_frame_union() {
        // Game mode / non-EVRTCK codecs must be unaffected by the EVRTCK routing
        // added to make_tcp_video_frame().
        use crate::rustdesk_proto::{peer_message, video_frame};

        let frame = EncodedFrame {
            bytes: std::sync::Arc::new(vec![0, 1, 2, 3]),
            is_idr: true,
            frame_id: 1,
            pts_us: 0,
            display: 0,
            sps_pps: None,
            width: 64,
            height: 64,
            codec: "H264",
            roi: crate::evrt::RoiRect {
                frame_id: 1,
                x: 0,
                y: 0,
                w: 0,
                h: 0,
            },
        };
        let msg = make_tcp_video_frame(&frame);
        match msg.union {
            Some(peer_message::Union::VideoFrame(vf)) => {
                assert!(matches!(vf.union, Some(video_frame::Union::H264s(_))));
            }
            other => panic!("expected VideoFrame, got {other:?}"),
        }
    }
}
