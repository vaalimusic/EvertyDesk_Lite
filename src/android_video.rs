// Android MediaCodec декодер — вызов Kotlin VideoDecoder.decodeFrame() через JNI.
//
// Surface output mode: MediaCodec рендерит декодированный кадр прямо в TextureView.
// Никаких пиксельных данных через JNI — GPU делает всё сам.
//
// ВАЖНО: env.find_class() из фонового потока использует системный ClassLoader,
// который не видит классы APK. Поэтому мы кешируем GlobalRef на VideoDecoder
// в JNI_OnLoad (на потоке приложения) и используем его здесь напрямую.

use jni::objects::{JClass, JObject, JValue};
use jni::sys::{jboolean, jint};
use std::sync::atomic::{AtomicU64, Ordering};

// Кешируем последнее значение чтобы не дёргать JNI на каждом тике feedback loop
// если PerfStats недоступен (до первого кадра). Обновляется раз в секунду.
static CACHED_DECODED_FRAMES: AtomicU64 = AtomicU64::new(0);
static CACHED_DECODE_MS: AtomicU64 = AtomicU64::new(0);

/// Декодировать кадр через Android MediaCodec в Surface (TextureView).
///
/// Возвращает true если кадр успешно передан декодеру, false при ошибке.
/// Поддерживаемые кодеки: "H264", "H265", "AV1".
pub fn decode_frame_to_surface(
    codec_name: &str,
    data: &[u8],
    is_keyframe: bool,
    width: u32,
    height: u32,
) -> bool {
    let Some(jvm) = crate::android_ffi::android_jvm() else {
        return false;
    };
    let Some(cls_ref) = crate::android_ffi::decoder_class_ref() else {
        return false;
    };
    let mut env = match jvm.attach_current_thread() {
        Ok(e) => e,
        Err(_) => return false,
    };

    let jcodec = match env.new_string(codec_name) {
        Ok(s) => s,
        Err(_) => return false,
    };
    let jdata = match env.byte_array_from_slice(data) {
        Ok(a) => a,
        Err(_) => return false,
    };

    // Используем закешированный GlobalRef — валиден на любом потоке.
    let cls: JClass = unsafe { JClass::from(JObject::from_raw(cls_ref.as_raw())) };

    let result = env.call_static_method(
        &cls,
        "decodeFrame",
        "(Ljava/lang/String;[BZII)Z",
        &[
            JValue::Object(&jcodec),
            JValue::Object(&jdata),
            JValue::Bool(is_keyframe as jboolean),
            JValue::Int(width as jint),
            JValue::Int(height as jint),
        ],
    );

    match result {
        Ok(v) => v.z().unwrap_or(false),
        Err(_) => false,
    }
}

/// Запрашивает у Android реальную HW-поддержку декодирования H265/AV1 через
/// `VideoDecoder.isDecodeSupported()` (обёртка над `MediaCodecList` пробой,
/// уже использовавшуюся для pre-warm/фильтрации декодера).
///
/// До этого `crate::video::h265_available()`/`av1_available()` на Android
/// были жёстко захардкожены в `false` с комментарием "MediaCodec в Kotlin
/// ещё не реализован" — хотя HW MediaCodec-декод уже реализован и работает
/// (см. VideoDecoder.kt). Из-за этого `preferred_codec()` в transport.rs
/// молча откатывал ЛЮБОЙ выбор пользователя (H265/AV1) на H264, независимо
/// от реальных возможностей телефона.
///
/// Возвращает (h265_supported, av1_supported). При недоступности JNI —
/// (false, false) — тот же консервативный дефолт, что был раньше.
pub fn query_android_decode_caps() -> (bool, bool) {
    let Some(jvm) = crate::android_ffi::android_jvm() else {
        return (false, false);
    };
    let Some(cls_ref) = crate::android_ffi::decoder_class_ref() else {
        return (false, false);
    };
    let mut env = match jvm.attach_current_thread() {
        Ok(e) => e,
        Err(_) => return (false, false),
    };
    let cls: JClass = unsafe { JClass::from(JObject::from_raw(cls_ref.as_raw())) };

    let h265_ok = match env.new_string("video/hevc") {
        Ok(jmime) => env
            .call_static_method(
                &cls,
                "isDecodeSupported",
                "(Ljava/lang/String;)Z",
                &[JValue::Object(&jmime)],
            )
            .and_then(|v| v.z())
            .unwrap_or(false),
        Err(_) => false,
    };
    let av1_ok = match env.new_string("video/av01") {
        Ok(jmime) => env
            .call_static_method(
                &cls,
                "isDecodeSupported",
                "(Ljava/lang/String;)Z",
                &[JValue::Object(&jmime)],
            )
            .and_then(|v| v.z())
            .unwrap_or(false),
        Err(_) => false,
    };
    (h265_ok, av1_ok)
}

/// Читает PerfStats.nativeTotalFrames() и nativeAvgDecodeMs() через два простых JNI-вызова.
/// Два отдельных вызова надёжнее, чем читать LongArray через unsafe JPrimitiveArray cast.
/// Возвращает (total_decoded_frames, avg_decode_ms). При ошибке — кешированные значения.
pub fn get_android_decode_stats() -> (u64, i32) {
    let cached = || {
        (
            CACHED_DECODED_FRAMES.load(Ordering::Relaxed),
            CACHED_DECODE_MS.load(Ordering::Relaxed) as i32,
        )
    };
    let Some(jvm) = crate::android_ffi::android_jvm() else {
        return cached();
    };
    let Some(cls_ref) = crate::android_ffi::perf_stats_class_ref() else {
        return cached();
    };
    let mut env = match jvm.attach_current_thread() {
        Ok(e) => e,
        Err(_) => return cached(),
    };
    let cls: JClass = unsafe { JClass::from(JObject::from_raw(cls_ref.as_raw())) };

    let frames = env
        .call_static_method(&cls, "nativeTotalFrames", "()J", &[])
        .and_then(|v| v.j())
        .unwrap_or(CACHED_DECODED_FRAMES.load(Ordering::Relaxed) as i64) as u64;

    let dec_ms = env
        .call_static_method(&cls, "nativeAvgDecodeMs", "()J", &[])
        .and_then(|v| v.j())
        .map(|v| v.clamp(0, i32::MAX as i64) as i32)
        .unwrap_or(CACHED_DECODE_MS.load(Ordering::Relaxed) as i32);

    CACHED_DECODED_FRAMES.store(frames, Ordering::Relaxed);
    CACHED_DECODE_MS.store(dec_ms as u64, Ordering::Relaxed);
    (frames, dec_ms)
}
