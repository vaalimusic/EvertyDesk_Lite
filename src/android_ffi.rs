// =============================================================================
// Android JNI-мост для EvertyDesk Lite (исходящие подключения / клиент).
//
// Kotlin вызывает эти функции через `external fun`. Rust держит сессию
// (transport::run_session в фоновом потоке), собирает последний RGBA-кадр,
// принимает тач-события → конвертирует в mouse-команды хоста.
//
// Имена функций: Java_<package>_<Class>_<method>
// Пакет: ru.everty.desklite, класс: NativeClient
// =============================================================================

use std::sync::{
    atomic::{AtomicBool, Ordering},
    mpsc::{self, Receiver, Sender},
    Arc, Mutex, OnceLock,
};
use std::thread;

use jni::objects::{GlobalRef, JByteArray, JClass, JString};
use jni::sys::{jboolean, jbyteArray, jint, jintArray, jlong};
use jni::{JNIEnv, JavaVM};

// ─── JavaVM и класс декодера — сохраняем при загрузке .so ─────────────────────

static ANDROID_JVM: OnceLock<JavaVM> = OnceLock::new();

// GlobalRef на VideoDecoder class — кешируем в JNI_OnLoad, пока доступен
// загрузчик классов приложения. Фоновые потоки используют этот ref напрямую,
// минуя системный загрузчик (который не знает о классах APK).
static DECODER_CLASS_REF: OnceLock<GlobalRef> = OnceLock::new();
static PERF_STATS_CLASS_REF: OnceLock<GlobalRef> = OnceLock::new();
// Класс службы доступности — для Android-хост режима (инжект ввода из Rust).
static INPUT_SERVICE_CLASS_REF: OnceLock<GlobalRef> = OnceLock::new();

/// Вызывается JVM при `System.loadLibrary("evertydesk_core")`.
/// Сохраняем JavaVM и GlobalRef на VideoDecoder/PerfStats для фоновых потоков.
#[no_mangle]
pub extern "system" fn JNI_OnLoad(
    vm: *mut jni::sys::JavaVM,
    _reserved: *mut std::ffi::c_void,
) -> jni::sys::jint {
    if let Ok(jvm) = unsafe { JavaVM::from_raw(vm) } {
        // Пока мы на потоке приложения — находим класс через правильный ClassLoader.
        if let Ok(mut env) = jvm.attach_current_thread() {
            if let Ok(cls) = env.find_class("ru/everty/desklite/VideoDecoder") {
                if let Ok(global) = env.new_global_ref(cls) {
                    let _ = DECODER_CLASS_REF.set(global);
                }
            }
            if let Ok(cls) = env.find_class("ru/everty/desklite/PerfStats") {
                if let Ok(global) = env.new_global_ref(cls) {
                    let _ = PERF_STATS_CLASS_REF.set(global);
                }
            }
            if let Ok(cls) = env.find_class("ru/everty/desklite/EvertyInputService") {
                if let Ok(global) = env.new_global_ref(cls) {
                    let _ = INPUT_SERVICE_CLASS_REF.set(global);
                }
            }
        }
        let _ = ANDROID_JVM.set(jvm);
    }
    jni::sys::JNI_VERSION_1_6
}

/// Доступ к JavaVM для фоновых JNI-вызовов (android_video.rs).
pub fn android_jvm() -> Option<&'static JavaVM> {
    ANDROID_JVM.get()
}

/// GlobalRef на класс VideoDecoder — безопасен для использования в любом потоке.
pub fn decoder_class_ref() -> Option<&'static GlobalRef> {
    DECODER_CLASS_REF.get()
}

/// GlobalRef на класс PerfStats — безопасен для использования в любом потоке.
pub fn perf_stats_class_ref() -> Option<&'static GlobalRef> {
    PERF_STATS_CLASS_REF.get()
}

/// GlobalRef на класс EvertyInputService — для Android-хост инжекта из любого потока.
pub fn input_service_class_ref() -> Option<&'static GlobalRef> {
    INPUT_SERVICE_CLASS_REF.get()
}

use crate::settings::{AppConfig, CodecPreference, ServerConfig};
use crate::transport::{
    ConnectionRequest, RemoteDisplay, SessionCommand, SessionEvent, TransportClient,
};

/// Последний полученный кадр (RGBA).
#[derive(Default)]
struct LatestFrame {
    width: u32,
    height: u32,
    rgba: Vec<u8>,
    /// Монотонный счётчик — клиент понимает, новый ли кадр.
    seq: u64,
}

#[derive(Clone, Copy, Debug, Default)]
struct RemoteBounds {
    x: i32,
    y: i32,
    width: u32,
    height: u32,
}

impl RemoteBounds {
    fn is_empty(self) -> bool {
        self.width == 0 || self.height == 0
    }
}

/// Состояние одной сессии. Указатель отдаётся в Kotlin как jlong-handle.
struct AndroidSession {
    cmd_tx: Sender<SessionCommand>,
    latest: Arc<Mutex<LatestFrame>>,
    remote_bounds: Arc<Mutex<RemoteBounds>>,
    stop: Arc<AtomicBool>,
    /// Текстовый статус (прогресс/ошибка) для UI.
    status: Arc<Mutex<String>>,
    connected: Arc<AtomicBool>,
    /// Экспериментальная кнопка «EVRT2»: отдельный слот кадра — сознательно
    /// НЕ делит `latest` с живым видео, иначе картинка мигала бы между
    /// EVRT1 и EVRT2. См. SessionEvent::Evrt2ExperimentFrame.
    evrt2_latest: Arc<Mutex<LatestFrame>>,
    /// ROADMAP.md Phase 3 live-test gap, found while verifying APF on a
    /// real device: `SessionEvent::Info` (every EVRT2 experimental status —
    /// "подключено", frame counts, MODE_SWITCH, APF confirmations, decode
    /// errors) was falling into `collect_events`'s catch-all and going
    /// nowhere visible on Android. Separate slot from `status` (which is
    /// the main connection's own status text) for the same reason
    /// `evrt2_latest` is separate from `latest` — mixing them would make
    /// one clobber the other on screen.
    evrt2_status: Arc<Mutex<String>>,
}

// ─── helper: лог ───────────────────────────────────────────────────────────────

fn jni_log(msg: &str) {
    log::info!("[evd-android] {msg}");
}

// ─── start ─────────────────────────────────────────────────────────────────────

/// Запустить сессию. Возвращает handle (указатель) или 0 при ошибке.
///
/// Kotlin: `external fun nativeStart(id, password, apiUrl, idServer, relayServer, publicKey, codec): Long`
#[no_mangle]
pub extern "system" fn Java_ru_everty_desklite_NativeClient_nativeStart(
    mut env: JNIEnv,
    _class: JClass,
    id: JString,
    password: JString,
    api_url: JString,
    id_server: JString,
    relay_server: JString,
    public_key: JString,
    codec: JString,
) -> jlong {
    start_android_session(
        &mut env,
        id,
        password,
        api_url,
        id_server,
        relay_server,
        public_key,
        codec,
        false,
    )
}

#[no_mangle]
pub extern "system" fn Java_ru_everty_desklite_NativeClient_nativeStartTouchpad(
    mut env: JNIEnv,
    _class: JClass,
    id: JString,
    password: JString,
    api_url: JString,
    id_server: JString,
    relay_server: JString,
    public_key: JString,
    codec: JString,
) -> jlong {
    start_android_session(
        &mut env,
        id,
        password,
        api_url,
        id_server,
        relay_server,
        public_key,
        codec,
        true,
    )
}

// ═══════════════════════════════════════════════════════════════════════════
// Android HOST режим (тачпад): устройство принимает подключения и инжектит
// ввод через службу доступности. Видео не захватывается — оператор смотрит на
// сам экран (например ТВ), телефон работает слепым трекпадом.
// ═══════════════════════════════════════════════════════════════════════════

static HOST_SERVICE: Mutex<Option<crate::host::HostService>> = Mutex::new(None);
static HOST_DRAIN_STARTED: AtomicBool = AtomicBool::new(false);
static HOST_LAST_STATUS: OnceLock<Mutex<String>> = OnceLock::new();

fn host_status_slot() -> &'static Mutex<String> {
    HOST_LAST_STATUS.get_or_init(|| Mutex::new("Ожидание запуска…".to_owned()))
}

/// Фоновый поток: сливает события хоста в лог/статус, чтобы канал не рос.
fn ensure_host_drain_thread() {
    if HOST_DRAIN_STARTED.swap(true, Ordering::SeqCst) {
        return;
    }
    thread::spawn(|| loop {
        {
            if let Ok(guard) = HOST_SERVICE.lock() {
                if let Some(svc) = guard.as_ref() {
                    while let Some(ev) = svc.try_recv() {
                        if let crate::host::HostEvent::Log(ref s) = ev {
                            if let Ok(mut st) = host_status_slot().lock() {
                                *st = s.clone();
                            }
                        }
                        jni_log(&format!("[host] {ev:?}"));
                    }
                }
            }
        }
        thread::sleep(std::time::Duration::from_millis(300));
    });
}

/// Запустить Android-хост (тачпад-режим). Возвращает true при старте.
///
/// Kotlin: `external fun nativeStartTouchpadHost(localId, password, idServer,
///          relayServer, publicKey, screenW, screenH): Boolean`
#[no_mangle]
pub extern "system" fn Java_ru_everty_desklite_NativeClient_nativeStartTouchpadHost(
    mut env: JNIEnv,
    _class: JClass,
    local_id: JString,
    password: JString,
    id_server: JString,
    relay_server: JString,
    public_key: JString,
    screen_w: jint,
    screen_h: jint,
) -> jboolean {
    init_android_logger();

    let get = |env: &mut JNIEnv, s: JString| -> String {
        env.get_string(&s).map(|v| v.into()).unwrap_or_default()
    };
    let local_id = get(&mut env, local_id);
    let password = get(&mut env, password);
    let id_server = get(&mut env, id_server);
    let relay_server = get(&mut env, relay_server);
    let public_key = get(&mut env, public_key);

    if local_id.is_empty() {
        jni_log("nativeStartTouchpadHost: пустой local_id");
        return 0;
    }

    crate::capture::set_android_screen(screen_w.max(1) as u32, screen_h.max(1) as u32);

    if !crate::android_input::is_ready() {
        jni_log("nativeStartTouchpadHost: EvertyInputService не найдена (служба доступности выключена?)");
    }

    let (sign_pk, sign_sk) = crate::crypto::gen_sign_keypair();
    let config = AppConfig {
        server: ServerConfig {
            api_url: "https://desk.everty.ru".to_owned(),
            id_server,
            relay_server,
            public_key,
        },
        local_id,
        local_password: password,
        permanent_password: String::new(),
        security: Default::default(),
        display: Default::default(),
        llm: Default::default(),
        ui: Default::default(),
        hotfix: Default::default(),
        udp_bind_port: 0,
        evrt_udp_port: 0,
        host_pk: Vec::new(),
        host_sign_pk: sign_pk,
        host_sign_sk: sign_sk,
    };

    let service = crate::host::HostService::start(config);
    if let Ok(mut guard) = HOST_SERVICE.lock() {
        if let Some(old) = guard.take() {
            old.stop();
        }
        *guard = Some(service);
    }
    ensure_host_drain_thread();
    jni_log("Android-хост (тачпад) запущен ✓");
    1
}

/// Остановить Android-хост.
/// Kotlin: `external fun nativeStopHost()`
#[no_mangle]
pub extern "system" fn Java_ru_everty_desklite_NativeClient_nativeStopHost(
    _env: JNIEnv,
    _class: JClass,
) {
    if let Ok(mut guard) = HOST_SERVICE.lock() {
        if let Some(svc) = guard.take() {
            svc.stop();
            jni_log("Android-хост остановлен");
        }
    }
}

/// Текущий статус хоста (последняя строка лога) для UI.
/// Kotlin: `external fun nativeHostStatus(): String`
#[no_mangle]
pub extern "system" fn Java_ru_everty_desklite_NativeClient_nativeHostStatus<'local>(
    env: JNIEnv<'local>,
    _class: JClass<'local>,
) -> jni::sys::jstring {
    let status = host_status_slot()
        .lock()
        .map(|s| s.clone())
        .unwrap_or_default();
    match env.new_string(status) {
        Ok(s) => s.into_raw(),
        Err(_) => std::ptr::null_mut(),
    }
}

fn parse_codec_preference(s: &str) -> CodecPreference {
    match s.trim().to_ascii_uppercase().as_str() {
        "H264" => CodecPreference::H264,
        "H265" => CodecPreference::H265,
        "AV1" => CodecPreference::Av1,
        "VP9" => CodecPreference::Vp9,
        "AUTO" => CodecPreference::Auto,
        _ => CodecPreference::Evrtck,
    }
}

fn start_android_session(
    env: &mut JNIEnv,
    id: JString,
    password: JString,
    api_url: JString,
    id_server: JString,
    relay_server: JString,
    public_key: JString,
    codec: JString,
    control_only: bool,
) -> jlong {
    // Инициализируем android_logger один раз
    init_android_logger();

    let remote_id: String = match env.get_string(&id) {
        Ok(s) => s.into(),
        Err(_) => return 0,
    };
    let password: String = match env.get_string(&password) {
        Ok(s) => s.into(),
        Err(_) => String::new(),
    };

    jni_log(&format!(
        "nativeStart id={remote_id} control_only={control_only}"
    ));

    let config = AppConfig::load_or_create();
    let server = ServerConfig {
        api_url: jstring_or(env, &api_url, config.server.api_url.clone()),
        id_server: jstring_or(env, &id_server, config.server.id_server.clone()),
        relay_server: jstring_or(env, &relay_server, config.server.relay_server.clone()),
        public_key: jstring_or(env, &public_key, config.server.public_key.clone()),
    };
    let mut display = config.display.clone();
    // Don't limit fps from the Android side — let the host enforce its own target_fps cap.
    // Stored config may have an outdated/low value; the host negotiates down if needed.
    display.target_fps = 60;
    // Game mode on LAN: disable adaptive quality so codec-switch noise doesn't
    // trigger lower_adaptive_fps(60→30) and send custom_fps=30 to the host.
    display.adaptive_quality = false;
    let codec_str: String = match env.get_string(&codec) {
        Ok(s) => s.into(),
        Err(_) => String::new(),
    };
    if !codec_str.trim().is_empty() {
        display.codec = parse_codec_preference(&codec_str);
    }
    // Query REAL device HW decode capability before advertising it to the
    // host. Previously h265/av1 support was hardcoded false on Android
    // (stale "not yet implemented" assumption), so any H265/AV1 selection
    // silently downgraded to H264 regardless of what the phone could
    // actually decode — see crate::video::set_android_decode_caps doc comment.
    let (h265_ok, av1_ok) = crate::android_video::query_android_decode_caps();
    crate::video::set_android_decode_caps(h265_ok, av1_ok);
    jni_log(&format!(
        "codec={:?} device_decode_caps: h265={h265_ok} av1={av1_ok}",
        display.codec
    ));
    let evrt2_only = codec_str.trim().eq_ignore_ascii_case("EVRT2ONLY");
    let request = ConnectionRequest {
        remote_id,
        password,
        client_id: "evertydesk-android".to_owned(),
        client_name: "EvertyDesk Android".to_owned(),
        server,
        display,
        control_only: control_only || evrt2_only,
        audio_enabled: Arc::new(AtomicBool::new(true)),
        evrt2_only,
        require_direct_transport: false,
    };

    let (cmd_tx, cmd_rx) = mpsc::channel::<SessionCommand>();
    let (ev_tx, ev_rx) = mpsc::channel::<SessionEvent>();

    let latest = Arc::new(Mutex::new(LatestFrame::default()));
    let evrt2_latest = Arc::new(Mutex::new(LatestFrame::default()));
    let evrt2_status = Arc::new(Mutex::new(String::new()));
    let status = Arc::new(Mutex::new("Подключение…".to_owned()));
    let connected = Arc::new(AtomicBool::new(false));
    let remote_bounds = Arc::new(Mutex::new(RemoteBounds::default()));
    let stop = Arc::new(AtomicBool::new(false));

    // Поток сессии
    {
        let stop_for_session = stop.clone();
        thread::spawn(move || {
            TransportClient::run_session(request, cmd_rx, ev_tx, stop_for_session);
        });
    }

    // Поток сбора событий → latest frame / status
    {
        let latest = latest.clone();
        let evrt2_latest = evrt2_latest.clone();
        let evrt2_status = evrt2_status.clone();
        let status = status.clone();
        let connected = connected.clone();
        let remote_bounds = remote_bounds.clone();
        let stop = stop.clone();
        thread::spawn(move || {
            collect_events(
                ev_rx,
                latest,
                evrt2_latest,
                evrt2_status,
                status,
                connected,
                remote_bounds,
                stop,
            );
        });
    }

    let session = Box::new(AndroidSession {
        cmd_tx,
        latest,
        remote_bounds,
        stop,
        status,
        connected,
        evrt2_latest,
        evrt2_status,
    });
    Box::into_raw(session) as jlong
}

fn collect_events(
    ev_rx: Receiver<SessionEvent>,
    latest: Arc<Mutex<LatestFrame>>,
    evrt2_latest: Arc<Mutex<LatestFrame>>,
    evrt2_status: Arc<Mutex<String>>,
    status: Arc<Mutex<String>>,
    connected: Arc<AtomicBool>,
    remote_bounds: Arc<Mutex<RemoteBounds>>,
    stop: Arc<AtomicBool>,
) {
    let mut seq = 0u64;
    let mut evrt2_seq = 0u64;
    let mut has_display_bounds = false;
    while !stop.load(Ordering::Relaxed) {
        match ev_rx.recv_timeout(std::time::Duration::from_millis(250)) {
            Ok(SessionEvent::Frame {
                width,
                height,
                rgba,
                ..
            }) => {
                seq += 1;
                if !has_display_bounds {
                    if let Ok(mut g) = remote_bounds.lock() {
                        *g = RemoteBounds {
                            x: 0,
                            y: 0,
                            width: width as u32,
                            height: height as u32,
                        };
                    }
                }
                if let Ok(mut f) = latest.lock() {
                    f.width = width as u32;
                    f.height = height as u32;
                    f.rgba = rgba;
                    f.seq = seq;
                }
            }
            Ok(SessionEvent::Evrt2ExperimentFrame {
                width,
                height,
                rgba,
            }) => {
                evrt2_seq += 1;
                if let Ok(mut f) = evrt2_latest.lock() {
                    f.width = width as u32;
                    f.height = height as u32;
                    f.rgba = rgba;
                    f.seq = evrt2_seq;
                }
            }
            Ok(SessionEvent::Connected(info)) => {
                connected.store(true, Ordering::Relaxed);
                if let Ok(mut s) = status.lock() {
                    *s = format!("Подключено: {info}");
                }
            }
            Ok(SessionEvent::Displays(displays)) => {
                if let Some(bounds) = remote_bounds_from_displays(&displays) {
                    if let Ok(mut g) = remote_bounds.lock() {
                        *g = bounds;
                        has_display_bounds = true;
                    }
                }
            }
            Ok(SessionEvent::Progress(pct, msg)) => {
                if let Ok(mut s) = status.lock() {
                    *s = format!("{pct}% {msg}");
                }
            }
            Ok(SessionEvent::Failed(err)) => {
                connected.store(false, Ordering::Relaxed);
                if let Ok(mut s) = status.lock() {
                    *s = format!("Ошибка: {err}");
                }
                break;
            }
            Ok(SessionEvent::Closed) => {
                connected.store(false, Ordering::Relaxed);
                if let Ok(mut s) = status.lock() {
                    *s = "Отключено".to_owned();
                }
                break;
            }
            Ok(SessionEvent::Info(msg)) => {
                // "EVRT2" prefix filters out unrelated Info traffic (shell
                // output, generic peer-message logs) so this slot only ever
                // shows the experimental stream's own status, matching the
                // filtering already implicit in evrt2_latest's separation
                // from the live-video `latest` slot.
                if msg.starts_with("EVRT2") {
                    if let Ok(mut s) = evrt2_status.lock() {
                        *s = msg;
                    }
                }
            }
            Ok(_) => {}
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(_) => break,
        }
    }
}

// ─── poll frame ──────────────────────────────────────────────────────────────

/// Скопировать последний кадр в `out` (IntArray ARGB) если есть новый.
/// Возвращает упакованный (width<<32 | height) или 0 если кадра нет.
///
/// Kotlin: `external fun nativePollFrame(handle: Long, out: IntArray): Long`
/// `out` должен быть достаточного размера (width*height). Клиент сначала
/// узнаёт размер через nativeFrameSize, аллоцирует, потом poll.
#[no_mangle]
pub extern "system" fn Java_ru_everty_desklite_NativeClient_nativePollFrame(
    env: JNIEnv,
    _class: JClass,
    handle: jlong,
    out: jintArray,
) -> jlong {
    let session = match session_ref(handle) {
        Some(s) => s,
        None => return 0,
    };

    // Снимаем снапшот кадра под коротким локом, сразу освобождаем мьютекс.
    // Это не блокирует collect_events пока идёт конвертация + JNI копия.
    let (w, h, rgba) = {
        let Ok(f) = session.latest.lock() else {
            return 0;
        };
        if f.seq == 0 || f.rgba.is_empty() {
            return 0;
        }
        (f.width as usize, f.height as usize, f.rgba.clone())
    };

    let px_count = w * h;
    if px_count == 0 {
        return 0;
    }

    let arr = unsafe { jni::objects::JIntArray::from_raw(out) };

    // Проверяем длину выходного массива ДО записи — защита от гонки когда
    // разрешение изменилось между nativeFrameSize() и nativePollFrame().
    let arr_len = match env.get_array_length(&arr) {
        Ok(l) => l as usize,
        Err(_) => {
            let _ = env.exception_clear();
            return 0;
        }
    };
    if arr_len < px_count {
        // Kotlin переаллоцирует на следующем тике по новому frameSize().
        return 0;
    }

    // Конвертируем RGBA(u8) → ARGB(i32) для Android Bitmap.Config.ARGB_8888.
    let mut argb = vec![0i32; px_count];
    for (i, chunk) in rgba.chunks_exact(4).take(px_count).enumerate() {
        let r = chunk[0] as i32;
        let g = chunk[1] as i32;
        let b = chunk[2] as i32;
        let a = chunk[3] as i32;
        argb[i] = (a << 24) | (r << 16) | (g << 8) | b;
    }

    if env.set_int_array_region(&arr, 0, &argb).is_err() {
        let _ = env.exception_clear();
        return 0;
    }
    ((w as i64) << 32) | (h as i64)
}

/// Размер последнего кадра: (width<<32 | height), или 0.
/// Kotlin: `external fun nativeFrameSize(handle: Long): Long`
#[no_mangle]
pub extern "system" fn Java_ru_everty_desklite_NativeClient_nativeFrameSize(
    _env: JNIEnv,
    _class: JClass,
    handle: jlong,
) -> jlong {
    let Some(session) = session_ref(handle) else {
        return 0;
    };
    let Ok(f) = session.latest.lock() else {
        return 0;
    };
    if f.seq == 0 {
        return 0;
    }
    ((f.width as i64) << 32) | (f.height as i64)
}

/// Экспериментальная кнопка «EVRT2»: размер последнего кадра из отдельного
/// EVRT2-потока (не пересекается с nativeFrameSize/nativePollFrame — см.
/// AndroidSession::evrt2_latest). (width<<32 | height), или 0.
/// Kotlin: `external fun nativeEvrt2FrameSize(handle: Long): Long`
#[no_mangle]
pub extern "system" fn Java_ru_everty_desklite_NativeClient_nativeEvrt2FrameSize(
    _env: JNIEnv,
    _class: JClass,
    handle: jlong,
) -> jlong {
    let Some(session) = session_ref(handle) else {
        return 0;
    };
    let Ok(f) = session.evrt2_latest.lock() else {
        return 0;
    };
    if f.seq == 0 {
        return 0;
    }
    ((f.width as i64) << 32) | (f.height as i64)
}

/// Экспериментальная кнопка «EVRT2»: скопировать последний EVRT2-кадр в
/// `out` (IntArray ARGB), аналогично nativePollFrame но из отдельного слота.
/// Kotlin: `external fun nativePollEvrt2Frame(handle: Long, out: IntArray): Long`
#[no_mangle]
pub extern "system" fn Java_ru_everty_desklite_NativeClient_nativePollEvrt2Frame(
    env: JNIEnv,
    _class: JClass,
    handle: jlong,
    out: jintArray,
) -> jlong {
    let session = match session_ref(handle) {
        Some(s) => s,
        None => return 0,
    };
    let (w, h, rgba) = {
        let Ok(f) = session.evrt2_latest.lock() else {
            return 0;
        };
        if f.seq == 0 || f.rgba.is_empty() {
            return 0;
        }
        (f.width as usize, f.height as usize, f.rgba.clone())
    };
    let px_count = w * h;
    if px_count == 0 {
        return 0;
    }

    let arr = unsafe { jni::objects::JIntArray::from_raw(out) };
    let arr_len = match env.get_array_length(&arr) {
        Ok(l) => l as usize,
        Err(_) => {
            let _ = env.exception_clear();
            return 0;
        }
    };
    if arr_len < px_count {
        return 0;
    }
    let mut argb = vec![0i32; px_count];
    for (i, chunk) in rgba.chunks_exact(4).take(px_count).enumerate() {
        let r = chunk[0] as i32;
        let g = chunk[1] as i32;
        let b = chunk[2] as i32;
        let a = chunk[3] as i32;
        argb[i] = (a << 24) | (r << 16) | (g << 8) | b;
    }
    if env.set_int_array_region(&arr, 0, &argb).is_err() {
        let _ = env.exception_clear();
        return 0;
    }
    ((w as i64) << 32) | (h as i64)
}

#[no_mangle]
pub extern "system" fn Java_ru_everty_desklite_NativeClient_nativeRemoteSize(
    _env: JNIEnv,
    _class: JClass,
    handle: jlong,
) -> jlong {
    let Some(session) = session_ref(handle) else {
        return 0;
    };
    let Ok(bounds) = session.remote_bounds.lock().map(|g| *g) else {
        return 0;
    };
    if bounds.is_empty() {
        return 0;
    }
    ((bounds.width as i64) << 32) | (bounds.height as i64)
}

#[no_mangle]
pub extern "system" fn Java_ru_everty_desklite_NativeClient_nativeRemoteOrigin(
    _env: JNIEnv,
    _class: JClass,
    handle: jlong,
) -> jlong {
    let Some(session) = session_ref(handle) else {
        return 0;
    };
    let Ok(bounds) = session.remote_bounds.lock().map(|g| *g) else {
        return 0;
    };
    pack_i32_pair(bounds.x, bounds.y)
}

// ─── touch → mouse ───────────────────────────────────────────────────────────

/// Тач-событие. action: 0=down, 1=move, 2=up.
/// x,y — в координатах удалённого экрана (Kotlin пересчитывает из view).
/// Kotlin: `external fun nativeTouch(handle: Long, x: Int, y: Int, action: Int)`
#[no_mangle]
pub extern "system" fn Java_ru_everty_desklite_NativeClient_nativeTouch(
    _env: JNIEnv,
    _class: JClass,
    handle: jlong,
    x: jint,
    y: jint,
    action: jint,
) {
    let Some(session) = session_ref(handle) else {
        return;
    };
    let cmd = match action {
        0 => SessionCommand::MouseDown { x, y },
        1 => SessionCommand::MouseMove { x, y },
        2 => SessionCommand::MouseUp { x, y },
        _ => return,
    };
    let _ = session.cmd_tx.send(cmd);
}

/// Правый клик (down + up). x,y — координаты удалённого экрана.
/// Kotlin: `external fun nativeRightClick(handle: Long, x: Int, y: Int)`
#[no_mangle]
pub extern "system" fn Java_ru_everty_desklite_NativeClient_nativeRightClick(
    _env: JNIEnv,
    _class: JClass,
    handle: jlong,
    x: jint,
    y: jint,
) {
    let Some(session) = session_ref(handle) else {
        return;
    };
    let _ = session.cmd_tx.send(SessionCommand::MouseMove { x, y });
    let _ = session.cmd_tx.send(SessionCommand::MouseRightDown { x, y });
    let _ = session.cmd_tx.send(SessionCommand::MouseRightUp { x, y });
}

/// Установить системную громкость хоста (0..100 %).
/// Kotlin: `external fun nativeSetHostVolume(handle: Long, volume: Int)`
#[no_mangle]
pub extern "system" fn Java_ru_everty_desklite_NativeClient_nativeSetHostVolume(
    _env: JNIEnv,
    _class: JClass,
    handle: jlong,
    volume: jint,
) {
    let Some(session) = session_ref(handle) else {
        return;
    };
    let vol = volume.clamp(0, 100) as u32;
    let _ = session
        .cmd_tx
        .send(crate::transport::SessionCommand::SetHostVolume(vol));
}

/// Экспериментальная кнопка «EVRT2» на игровом экране — просит хост поднять
/// отдельный EVRT2 UDP-сокет параллельно живой сессии.
/// Kotlin: `external fun nativeStartEvrt2Experiment(handle: Long)`
#[no_mangle]
pub extern "system" fn Java_ru_everty_desklite_NativeClient_nativeStartEvrt2Experiment(
    _env: JNIEnv,
    _class: JClass,
    handle: jlong,
) {
    let Some(session) = session_ref(handle) else {
        return;
    };
    let _ = session
        .cmd_tx
        .send(crate::transport::SessionCommand::StartEvrt2Experiment);
}

/// Ввод текста (Unicode-строка). Kotlin: `external fun nativeKeyText(handle: Long, text: String)`
#[no_mangle]
pub extern "system" fn Java_ru_everty_desklite_NativeClient_nativeKeyText(
    mut env: JNIEnv,
    _class: JClass,
    handle: jlong,
    text: JString,
) {
    let Some(session) = session_ref(handle) else {
        return;
    };
    let s: String = match env.get_string(&text) {
        Ok(v) => v.into(),
        Err(_) => return,
    };
    let _ = session
        .cmd_tx
        .send(crate::transport::SessionCommand::KeyText(s));
}

/// Управляющая клавиша по числовому коду ControlKey.
/// Kotlin: `external fun nativeKeyControl(handle: Long, code: Int)`
#[no_mangle]
pub extern "system" fn Java_ru_everty_desklite_NativeClient_nativeKeyControl(
    _env: JNIEnv,
    _class: JClass,
    handle: jlong,
    code: jint,
) {
    let Some(session) = session_ref(handle) else {
        return;
    };
    use crate::rustdesk_proto::ControlKey;
    let key = ControlKey::try_from(code).unwrap_or(ControlKey::Unknown);
    if key != ControlKey::Unknown {
        let _ = session
            .cmd_tx
            .send(crate::transport::SessionCommand::KeyControl(key));
    }
}

/// Навигация браузера жестом (3 пальца): Alt+← (назад) или Alt+→ (вперёд).
/// forward=false → назад, true → вперёд.
/// Kotlin: `external fun nativeNavigate(handle: Long, forward: Boolean)`
#[no_mangle]
pub extern "system" fn Java_ru_everty_desklite_NativeClient_nativeNavigate(
    _env: JNIEnv,
    _class: JClass,
    handle: jlong,
    forward: jboolean,
) {
    let Some(session) = session_ref(handle) else {
        return;
    };
    use crate::rustdesk_proto::ControlKey;
    let key = if forward != 0 {
        ControlKey::RightArrow
    } else {
        ControlKey::LeftArrow
    };
    let _ = session
        .cmd_tx
        .send(crate::transport::SessionCommand::KeyControlWithModifiers {
            key,
            modifiers: vec![ControlKey::Alt],
        });
}

/// Ctrl+символ. Kotlin: `external fun nativeKeyCtrl(handle: Long, ch: String)`
#[no_mangle]
pub extern "system" fn Java_ru_everty_desklite_NativeClient_nativeKeyCtrl(
    mut env: JNIEnv,
    _class: JClass,
    handle: jlong,
    ch: JString,
) {
    let Some(session) = session_ref(handle) else {
        return;
    };
    let s: String = match env.get_string(&ch) {
        Ok(v) => v.into(),
        Err(_) => return,
    };
    use crate::rustdesk_proto::ControlKey;
    use crate::transport::SessionCommand;
    let _ = session.cmd_tx.send(SessionCommand::KeyTextWithModifiers {
        text: s,
        modifiers: vec![ControlKey::Control],
    });
}

/// Прокрутка колеса мыши. delta_y > 0 — вниз, < 0 — вверх (единицы: условные шаги).
/// Kotlin: `external fun nativeScroll(handle: Long, x: Int, y: Int, deltaY: Int)`
#[no_mangle]
pub extern "system" fn Java_ru_everty_desklite_NativeClient_nativeScroll(
    _env: JNIEnv,
    _class: JClass,
    handle: jlong,
    x: jint,
    _y: jint,
    delta_y: jint,
) {
    let Some(session) = session_ref(handle) else {
        return;
    };
    let _ = session
        .cmd_tx
        .send(SessionCommand::MouseWheel { x, y: delta_y });
}

// ─── status / connected ──────────────────────────────────────────────────────

/// Текущий статус (строка). Kotlin: `external fun nativeStatus(handle: Long): String`
#[no_mangle]
pub extern "system" fn Java_ru_everty_desklite_NativeClient_nativeStatus<'local>(
    env: JNIEnv<'local>,
    _class: JClass,
    handle: jlong,
) -> jni::sys::jstring {
    let text = session_ref(handle)
        .and_then(|s| s.status.lock().ok().map(|g| g.clone()))
        .unwrap_or_else(|| "—".to_owned());
    match env.new_string(text) {
        Ok(s) => s.into_raw(),
        Err(_) => std::ptr::null_mut(),
    }
}

/// Последний статус EVRT2-эксперимента (пусто, если ничего ещё не пришло).
/// Kotlin: `external fun nativeEvrt2Status(handle: Long): String`
#[no_mangle]
pub extern "system" fn Java_ru_everty_desklite_NativeClient_nativeEvrt2Status<'local>(
    env: JNIEnv<'local>,
    _class: JClass,
    handle: jlong,
) -> jni::sys::jstring {
    let text = session_ref(handle)
        .and_then(|s| s.evrt2_status.lock().ok().map(|g| g.clone()))
        .unwrap_or_default();
    match env.new_string(text) {
        Ok(s) => s.into_raw(),
        Err(_) => std::ptr::null_mut(),
    }
}

/// Подключено ли. Kotlin: `external fun nativeIsConnected(handle: Long): Boolean`
#[no_mangle]
pub extern "system" fn Java_ru_everty_desklite_NativeClient_nativeIsConnected(
    _env: JNIEnv,
    _class: JClass,
    handle: jlong,
) -> jboolean {
    let v = session_ref(handle)
        .map(|s| s.connected.load(Ordering::Relaxed))
        .unwrap_or(false);
    jboolean::from(v)
}

// ─── resolution hint ─────────────────────────────────────────────────────────

/// Сообщить хосту максимальное разрешение экрана клиента (до старта сессии).
/// Kotlin: `external fun nativeSetMaxResolution(width: Int, height: Int)`
#[no_mangle]
pub extern "system" fn Java_ru_everty_desklite_NativeClient_nativeSetMaxResolution(
    _env: JNIEnv,
    _class: JClass,
    width: jint,
    height: jint,
) {
    crate::evrt_client::set_max_resolution(width as u32, height as u32);
}

// ─── stop ──────────────────────────────────────────────────────────────────────

/// Остановить и освободить сессию. Kotlin: `external fun nativeStop(handle: Long)`
#[no_mangle]
pub extern "system" fn Java_ru_everty_desklite_NativeClient_nativeStop(
    _env: JNIEnv,
    _class: JClass,
    handle: jlong,
) {
    if handle == 0 {
        return;
    }
    let session = unsafe { Box::from_raw(handle as *mut AndroidSession) };
    session.stop.store(true, Ordering::Relaxed);
    let _ = session.cmd_tx.send(SessionCommand::Close);
    jni_log("nativeStop");
    // Box drop освобождает память
}

// ─── helpers ──────────────────────────────────────────────────────────────────

/// Безопасная ссылка на сессию по handle (без передачи владения).
fn remote_bounds_from_displays(displays: &[RemoteDisplay]) -> Option<RemoteBounds> {
    let mut any = false;
    let mut min_x = i64::MAX;
    let mut min_y = i64::MAX;
    let mut max_x = i64::MIN;
    let mut max_y = i64::MIN;

    for display in displays {
        if display.width <= 0 || display.height <= 0 {
            continue;
        }

        any = true;
        let x0 = i64::from(display.x);
        let y0 = i64::from(display.y);
        let x1 = x0 + i64::from(display.width);
        let y1 = y0 + i64::from(display.height);
        min_x = min_x.min(x0);
        min_y = min_y.min(y0);
        max_x = max_x.max(x1);
        max_y = max_y.max(y1);
    }

    if !any || max_x <= min_x || max_y <= min_y {
        return None;
    }

    Some(RemoteBounds {
        x: i32::try_from(min_x).ok()?,
        y: i32::try_from(min_y).ok()?,
        width: u32::try_from(max_x - min_x).ok()?,
        height: u32::try_from(max_y - min_y).ok()?,
    })
}

fn session_ref<'a>(handle: jlong) -> Option<&'a AndroidSession> {
    if handle == 0 {
        None
    } else {
        Some(unsafe { &*(handle as *const AndroidSession) })
    }
}

fn pack_i32_pair(x: i32, y: i32) -> jlong {
    ((i64::from(x)) << 32) | (i64::from(y) & 0xFFFF_FFFF)
}

fn init_android_logger() {
    use std::sync::Once;
    static INIT: Once = Once::new();
    INIT.call_once(|| {
        android_logger::init_once(
            android_logger::Config::default()
                .with_max_level(log::LevelFilter::Info)
                .with_tag("EvertyDesk"),
        );
    });
}

fn jstring_or(env: &mut JNIEnv, value: &JString, fallback: String) -> String {
    match env.get_string(value) {
        Ok(raw) => {
            let text: String = raw.into();
            if text.trim().is_empty() {
                fallback
            } else {
                text
            }
        }
        Err(_) => fallback,
    }
}

// ─── аудио ───────────────────────────────────────────────────────────────────

/// Достать один PCM фрейм из очереди. Возвращает null если нет данных.
/// Формат: PCM 16-bit stereo, little-endian. Sample rate — см. nativeGetAudioSampleRate.
/// Kotlin: `external fun nativePollAudio(handle: Long): ByteArray?`
#[no_mangle]
pub extern "system" fn Java_ru_everty_desklite_NativeClient_nativePollAudio(
    env: JNIEnv,
    _class: JClass,
    _handle: jlong,
) -> jbyteArray {
    let Some(pcm) = crate::evrt_audio::pop_android_audio() else {
        return std::ptr::null_mut();
    };
    match env.byte_array_from_slice(&pcm) {
        Ok(arr) => arr.into_raw(),
        Err(_) => std::ptr::null_mut(),
    }
}

/// Получить sample rate аудио потока (из AudioConfig от хоста). Default 48000.
/// Kotlin: `external fun nativeGetAudioSampleRate(): Int`
#[no_mangle]
pub extern "system" fn Java_ru_everty_desklite_NativeClient_nativeGetAudioSampleRate(
    _env: JNIEnv,
    _class: JClass,
) -> jint {
    crate::evrt_audio::get_audio_sample_rate() as jint
}

/// Текущая глубина аудио очереди в фреймах. Используется jitter buffer на стороне Kotlin.
/// Kotlin: `external fun nativeAudioQueueDepth(): Int`
#[no_mangle]
pub extern "system" fn Java_ru_everty_desklite_NativeClient_nativeAudioQueueDepth(
    _env: JNIEnv,
    _class: JClass,
) -> jint {
    crate::evrt_audio::android_queue_depth() as jint
}
