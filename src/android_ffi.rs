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
    Arc, Mutex,
};
use std::thread;

use jni::objects::{JClass, JString};
use jni::sys::{jboolean, jint, jintArray, jlong};
use jni::JNIEnv;

use crate::settings::AppConfig;
use crate::transport::{ConnectionRequest, SessionCommand, SessionEvent, TransportClient};

/// Последний полученный кадр (RGBA).
#[derive(Default)]
struct LatestFrame {
    width: u32,
    height: u32,
    rgba: Vec<u8>,
    /// Монотонный счётчик — клиент понимает, новый ли кадр.
    seq: u64,
}

/// Состояние одной сессии. Указатель отдаётся в Kotlin как jlong-handle.
struct AndroidSession {
    cmd_tx: Sender<SessionCommand>,
    latest: Arc<Mutex<LatestFrame>>,
    stop: Arc<AtomicBool>,
    /// Текстовый статус (прогресс/ошибка) для UI.
    status: Arc<Mutex<String>>,
    connected: Arc<AtomicBool>,
    /// Размер удалённого экрана (для пересчёта координат тача).
    remote_size: Arc<Mutex<(u32, u32)>>,
}

// ─── helper: лог ───────────────────────────────────────────────────────────────

fn jni_log(msg: &str) {
    log::info!("[evd-android] {msg}");
}

// ─── start ─────────────────────────────────────────────────────────────────────

/// Запустить сессию. Возвращает handle (указатель) или 0 при ошибке.
///
/// Kotlin: `external fun nativeStart(id: String, password: String): Long`
#[no_mangle]
pub extern "system" fn Java_ru_everty_desklite_NativeClient_nativeStart(
    mut env: JNIEnv,
    _class: JClass,
    id: JString,
    password: JString,
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

    jni_log(&format!("nativeStart id={remote_id}"));

    let config = AppConfig::load_or_create();
    let request = ConnectionRequest {
        remote_id,
        password,
        server: config.server.clone(),
        display: config.display.clone(),
    };

    let (cmd_tx, cmd_rx) = mpsc::channel::<SessionCommand>();
    let (ev_tx, ev_rx) = mpsc::channel::<SessionEvent>();

    let latest = Arc::new(Mutex::new(LatestFrame::default()));
    let status = Arc::new(Mutex::new("Подключение…".to_owned()));
    let connected = Arc::new(AtomicBool::new(false));
    let remote_size = Arc::new(Mutex::new((0u32, 0u32)));
    let stop = Arc::new(AtomicBool::new(false));

    // Поток сессии
    {
        thread::spawn(move || {
            TransportClient::run_session(request, cmd_rx, ev_tx);
        });
    }

    // Поток сбора событий → latest frame / status
    {
        let latest = latest.clone();
        let status = status.clone();
        let connected = connected.clone();
        let remote_size = remote_size.clone();
        let stop = stop.clone();
        thread::spawn(move || {
            collect_events(ev_rx, latest, status, connected, remote_size, stop);
        });
    }

    let session = Box::new(AndroidSession {
        cmd_tx,
        latest,
        stop,
        status,
        connected,
        remote_size,
    });
    Box::into_raw(session) as jlong
}

fn collect_events(
    ev_rx: Receiver<SessionEvent>,
    latest: Arc<Mutex<LatestFrame>>,
    status: Arc<Mutex<String>>,
    connected: Arc<AtomicBool>,
    remote_size: Arc<Mutex<(u32, u32)>>,
    stop: Arc<AtomicBool>,
) {
    let mut seq = 0u64;
    while !stop.load(Ordering::Relaxed) {
        match ev_rx.recv_timeout(std::time::Duration::from_millis(250)) {
            Ok(SessionEvent::Frame { width, height, rgba, .. }) => {
                seq += 1;
                if let Ok(mut g) = remote_size.lock() {
                    *g = (width as u32, height as u32);
                }
                if let Ok(mut f) = latest.lock() {
                    f.width = width as u32;
                    f.height = height as u32;
                    f.rgba = rgba;
                    f.seq = seq;
                }
            }
            Ok(SessionEvent::Connected(info)) => {
                connected.store(true, Ordering::Relaxed);
                if let Ok(mut s) = status.lock() {
                    *s = format!("Подключено: {info}");
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
    mut env: JNIEnv,
    _class: JClass,
    handle: jlong,
    out: jintArray,
) -> jlong {
    let session = match session_ref(handle) {
        Some(s) => s,
        None => return 0,
    };
    let Ok(f) = session.latest.lock() else { return 0 };
    if f.seq == 0 || f.rgba.is_empty() {
        return 0;
    }
    let w = f.width as usize;
    let h = f.height as usize;
    let px_count = w * h;

    // Конвертируем RGBA(u8) → ARGB(i32) для Android Bitmap.Config.ARGB_8888.
    let mut argb = vec![0i32; px_count];
    for (i, chunk) in f.rgba.chunks_exact(4).take(px_count).enumerate() {
        let r = chunk[0] as i32;
        let g = chunk[1] as i32;
        let b = chunk[2] as i32;
        let a = chunk[3] as i32;
        argb[i] = (a << 24) | (r << 16) | (g << 8) | b;
    }

    // out — jintArray; копируем
    let arr = unsafe { jni::objects::JIntArray::from_raw(out) };
    if env.set_int_array_region(&arr, 0, &argb).is_err() {
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
    let Some(session) = session_ref(handle) else { return 0 };
    let Ok(f) = session.latest.lock() else { return 0 };
    if f.seq == 0 {
        return 0;
    }
    ((f.width as i64) << 32) | (f.height as i64)
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
    let Some(session) = session_ref(handle) else { return };
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
    let Some(session) = session_ref(handle) else { return };
    let _ = session.cmd_tx.send(SessionCommand::MouseMove { x, y });
    let _ = session.cmd_tx.send(SessionCommand::MouseRightDown { x, y });
    let _ = session.cmd_tx.send(SessionCommand::MouseRightUp { x, y });
}

/// Прокрутка колеса мыши. delta_y > 0 — вниз, < 0 — вверх (единицы: условные шаги).
/// Kotlin: `external fun nativeScroll(handle: Long, x: Int, y: Int, deltaY: Int)`
#[no_mangle]
pub extern "system" fn Java_ru_everty_desklite_NativeClient_nativeScroll(
    _env: JNIEnv,
    _class: JClass,
    handle: jlong,
    x: jint,
    y: jint,
    delta_y: jint,
) {
    let Some(session) = session_ref(handle) else { return };
    let _ = session.cmd_tx.send(SessionCommand::MouseWheel { x, y: delta_y });
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
fn session_ref<'a>(handle: jlong) -> Option<&'a AndroidSession> {
    if handle == 0 {
        None
    } else {
        Some(unsafe { &*(handle as *const AndroidSession) })
    }
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
