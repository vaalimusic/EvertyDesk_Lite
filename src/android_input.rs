// =============================================================================
// Android host input bridge — маршрутизация ввода из Rust-хоста в Kotlin-службу
// доступности (EvertyInputService), которая физически инжектит жесты в Android.
//
// Поток: relay_session_inner (host.rs) декодирует MouseEvent/KeyEvent от клиента
// → inject_mouse/inject_key (host.rs, cfg android) → сюда → JNI-вызов статического
// метода EvertyInputService.onMouseNative / onKeyNative.
//
// JavaVM и GlobalRef на класс службы кэшируются в android_ffi::JNI_OnLoad (на
// потоке приложения, где find_class видит классы APK). Здесь мы лишь берём их и
// attach_current_thread — хост крутится в своём Rust-потоке.
// =============================================================================

use jni::objects::{JClass, JValue};

/// true если служба доступности зарегистрирована (класс найден в JNI_OnLoad).
pub fn is_ready() -> bool {
    crate::android_ffi::input_service_class_ref().is_some()
}

/// Форвард события мыши в Kotlin-службу.
///   kind:   0=move, 1=down, 2=up, 3=wheel
///   button: 1=left, 2=right, 4=wheel (для wheel не используется)
///   x, y:   для move/down/up — абсолютные координаты в пространстве экрана хоста;
///           для wheel — дельты прокрутки (x, y).
pub fn on_mouse(kind: i32, button: i32, x: i32, y: i32) {
    call_static(
        "onMouseNative",
        "(IIII)V",
        &[
            JValue::Int(kind),
            JValue::Int(button),
            JValue::Int(x),
            JValue::Int(y),
        ],
    );
}

/// Форвард управляющей клавиши в Kotlin-службу (маппится на глобальные действия
/// доступности: BACK / HOME / RECENTS и т.п.).
///   code: ControlKey как i32 (см. rustdesk_proto::ControlKey)
///   alt:  зажат ли Alt (для Alt+←/→ навигации)
pub fn on_key(code: i32, alt: bool) {
    call_static(
        "onKeyNative",
        "(IZ)V",
        &[JValue::Int(code), JValue::Bool(alt as u8)],
    );
}

fn call_static(name: &str, sig: &str, args: &[JValue]) {
    let Some(vm) = crate::android_ffi::android_jvm() else {
        return;
    };
    let Some(cls_ref) = crate::android_ffi::input_service_class_ref() else {
        return;
    };
    let Ok(mut env) = vm.attach_current_thread() else {
        return;
    };
    // GlobalRef хранит JObject класса — оборачиваем сырой указатель в JClass.
    let cls: JClass = unsafe { JClass::from_raw(cls_ref.as_obj().as_raw()) };
    let _ = env.call_static_method(&cls, name, sig, args);
}
